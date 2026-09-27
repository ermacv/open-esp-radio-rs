//! Comparison of captured vendor and production calibrations under the
//! reviewed `phy_param` relation and reviewed tolerances.
//!
//! Each compared field element passes when every production capture lies
//! within the range the vendor captures span, widened by the field's
//! reviewed margin: cold calibrations differ from boot to boot, so the
//! vendor's own spread on the same board is the reference. A field without
//! a reviewed tolerance leaves the comparison INCOMPLETE; tolerances naming
//! no compared field are rejected.
use crate::capture::{Capture, PRODUCTION_PREFIX, VENDOR_PREFIX};
use crate::committed::{
    CALIBRATION, CALIBRATION_BYTES, OutputField, TRACKING_PROGRESS_FIELD, VENDOR_OBJECT, committed,
};
use crate::{Result, production, repository_root, vendor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Summary format.
const SCHEMA: u16 = 1;
/// Tolerance file format.
const TOLERANCE_SCHEMA: u16 = 1;
/// Reviewed tolerances, relative to this package.
const TOLERANCES: &str = "tolerances.toml";
/// Tracked summary, relative to the repository root.
const SUMMARY: &str = "verification/esp32s31/evidence/hardware/calibration.json";
const SECONDS_PER_DAY: u64 = 86_400;

#[derive(clap::Args)]
pub struct Arguments {
    /// Directory written by `capture`.
    #[arg(long)]
    captures: PathBuf,
    #[arg(long)]
    tolerances: Option<PathBuf>,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tolerances {
    schema: u16,
    #[serde(default)]
    fields: BTreeMap<String, Tolerance>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tolerance {
    /// Whether the field's elements are two's-complement values.
    signed: bool,
    /// Allowed distance beyond the vendor range, in element units.
    margin: u32,
    reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Match,
    Diff,
    Incomplete,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Summary {
    schema: u16,
    /// UTC day the captures started.
    date: String,
    verdict: Verdict,
    vendor_object: &'static str,
    vendor_application_sha256: String,
    vendor_idf_revision: String,
    vendor_captures: usize,
    production_image: String,
    production_application_sha256: String,
    production_captures: usize,
    fields: Vec<FieldSummary>,
    /// Fields of the relation that are not calibration.
    excluded: Vec<&'static str>,
    /// Vendor object byte ranges no compared field covers, `[start, end)`.
    uncovered: Vec<[usize; 2]>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct FieldSummary {
    name: &'static str,
    verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    margin: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// Per element, the vendor and production `[min, max]`.
    vendor: Vec<[i64; 2]>,
    production: Vec<[i64; 2]>,
}

/// The fields the cross-check compares: the relation without its tracking
/// progress, which counts tracking work rather than calibration.
fn fields() -> Vec<OutputField> {
    CALIBRATION
        .into_iter()
        .chain(committed(CALIBRATION_BYTES))
        .filter(|f| f.name != TRACKING_PROGRESS_FIELD)
        .collect()
}

/// Element `index` of `field` in `bytes` at `offset`.
fn element(
    bytes: &[u8],
    offset: u32,
    field: &OutputField,
    index: u32,
    signed: bool,
) -> Result<i64> {
    let width = usize::from(field.width);
    let start = offset as usize + index as usize * width;
    let slice = bytes
        .get(start..start + width)
        .ok_or_else(|| format!("{} lies outside the captured bytes", field.name))?;
    let mut raw = [0; 8];
    raw[..width].copy_from_slice(slice);
    let value = u64::from_le_bytes(raw);
    let bits = 8 * width as u32;
    Ok(if signed && value >> (bits - 1) & 1 == 1 {
        value as i64 - (1_i64 << bits)
    } else {
        value as i64
    })
}

fn range(values: impl Iterator<Item = i64>) -> [i64; 2] {
    values.fold([i64::MAX, i64::MIN], |[low, high], v| {
        [low.min(v), high.max(v)]
    })
}

fn compare_field(
    field: &OutputField,
    tolerance: Option<&Tolerance>,
    vendor: &[Vec<u8>],
    production: &[Vec<u8>],
) -> Result<FieldSummary> {
    let signed = tolerance.is_some_and(|t| t.signed);
    let (mut vendor_ranges, mut production_ranges) = (vec![], vec![]);
    let mut within = true;
    for index in 0..field.count {
        let v = range(
            vendor
                .iter()
                .map(|bytes| element(bytes, field.parameter, field, index, signed))
                .collect::<Result<Vec<_>>>()?
                .into_iter(),
        );
        let p = range(
            production
                .iter()
                .map(|bytes| element(bytes, field.output, field, index, signed))
                .collect::<Result<Vec<_>>>()?
                .into_iter(),
        );
        let margin = i64::from(tolerance.map_or(0, |t| t.margin));
        within &= v[0] - margin <= p[0] && p[1] <= v[1] + margin;
        vendor_ranges.push(v);
        production_ranges.push(p);
    }
    Ok(FieldSummary {
        name: field.name,
        verdict: match (tolerance, within) {
            (None, _) => Verdict::Incomplete,
            (Some(_), true) => Verdict::Match,
            (Some(_), false) => Verdict::Diff,
        },
        margin: tolerance.map(|t| t.margin),
        reason: tolerance.map(|t| t.reason.clone()),
        vendor: vendor_ranges,
        production: production_ranges,
    })
}

/// Vendor byte ranges of an object of `length` bytes that no field covers.
fn uncovered(length: usize, fields: &[OutputField]) -> Vec<[usize; 2]> {
    let mut covered = vec![false; length];
    for field in fields {
        let start = field.parameter as usize;
        let end = (start + usize::from(field.width) * field.count as usize).min(length);
        covered[start.min(end)..end].fill(true);
    }
    let mut ranges: Vec<[usize; 2]> = vec![];
    for (offset, covered) in covered.into_iter().enumerate() {
        if covered {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last[1] == offset => last[1] += 1,
            _ => ranges.push([offset, offset + 1]),
        }
    }
    ranges
}

/// `YYYY-MM-DD` of the UTC day holding `unix_seconds` (civil-from-days).
fn utc_date(unix_seconds: u64) -> String {
    let days = (unix_seconds / SECONDS_PER_DAY) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn numbered(directory: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut paths = std::fs::read_dir(directory)?
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    paths.retain(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(prefix))
    });
    paths.sort();
    Ok(paths)
}

pub fn run(arguments: &Arguments) -> Result<std::process::ExitCode> {
    let root = repository_root();
    let tolerance_path = arguments
        .tolerances
        .clone()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TOLERANCES));
    let tolerances: Tolerances = toml::from_str(&std::fs::read_to_string(&tolerance_path)?)?;
    if tolerances.schema != TOLERANCE_SCHEMA {
        return Err(format!("unsupported tolerance schema {}", tolerances.schema).into());
    }
    let fields = fields();
    if let Some(unknown) = tolerances
        .fields
        .keys()
        .find(|name| !fields.iter().any(|f| f.name == name.as_str()))
    {
        return Err(format!("tolerance for {unknown}, which the relation does not compare").into());
    }
    let capture: Capture = serde_json::from_slice(&std::fs::read(
        arguments.captures.join(crate::capture::RECORD),
    )?)?;
    let vendor = numbered(&arguments.captures, VENDOR_PREFIX)?
        .iter()
        .map(|path| {
            let mut objects = vendor::parse(&std::fs::read_to_string(path)?)?;
            objects
                .remove(VENDOR_OBJECT)
                .ok_or_else(|| format!("{} lacks {VENDOR_OBJECT}", path.display()).into())
        })
        .collect::<Result<Vec<_>>>()?;
    let production = numbered(&arguments.captures, PRODUCTION_PREFIX)?
        .iter()
        .map(|path| production::output(&std::fs::read(path)?))
        .collect::<Result<Vec<_>>>()?;
    if vendor.is_empty() || production.is_empty() {
        return Err("the captures hold no vendor or no production calibration".into());
    }
    let length = vendor[0].len();
    if vendor.iter().any(|bytes| bytes.len() != length) {
        return Err(format!("{VENDOR_OBJECT} captures differ in length").into());
    }
    let summaries = fields
        .iter()
        .map(|field| {
            compare_field(
                field,
                tolerances.fields.get(field.name),
                &vendor,
                &production,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let verdict = if summaries.iter().any(|f| f.verdict == Verdict::Diff) {
        Verdict::Diff
    } else if summaries.iter().any(|f| f.verdict == Verdict::Incomplete) {
        Verdict::Incomplete
    } else {
        Verdict::Match
    };
    let summary = Summary {
        schema: SCHEMA,
        date: utc_date(capture.started_unix_seconds),
        verdict,
        vendor_object: VENDOR_OBJECT,
        vendor_application_sha256: capture.vendor_application_sha256,
        vendor_idf_revision: capture.vendor_idf_revision,
        vendor_captures: vendor.len(),
        production_image: capture.production_image,
        production_application_sha256: capture.production_application_sha256,
        production_captures: production.len(),
        fields: summaries,
        excluded: vec![TRACKING_PROGRESS_FIELD],
        uncovered: uncovered(length, &fields),
    };
    let output = arguments
        .output
        .clone()
        .unwrap_or_else(|| root.join(SUMMARY));
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(&summary)?;
    bytes.push(b'\n');
    std::fs::write(&output, bytes)?;
    for field in &summary.fields {
        println!("{:<24} {:?}", field.name, field.verdict);
    }
    println!(
        "calibration cross-check {:?}: {}",
        summary.verdict,
        output.display()
    );
    Ok(if summary.verdict == Verdict::Match {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::committed::field;

    fn tolerance(margin: u32) -> Tolerance {
        Tolerance {
            signed: true,
            margin,
            reason: String::new(),
        }
    }

    #[test]
    fn production_within_the_widened_vendor_range_matches() {
        let field = field("value", 0, 1, 1, 1);
        let vendor = [vec![10], vec![12]];
        let production = [vec![0, 13]];
        let exact = compare_field(&field, Some(&tolerance(0)), &vendor, &production).unwrap();
        assert_eq!(exact.verdict, Verdict::Diff);
        let widened = compare_field(&field, Some(&tolerance(1)), &vendor, &production).unwrap();
        assert_eq!(widened.verdict, Verdict::Match);
        assert_eq!(
            (widened.vendor[0], widened.production[0]),
            ([10, 12], [13, 13])
        );
        let unreviewed = compare_field(&field, None, &vendor, &production).unwrap();
        assert_eq!(unreviewed.verdict, Verdict::Incomplete);
    }

    #[test]
    fn signed_elements_extend_their_sign() {
        let field = field("temperature", 0, 0, 2, 1);
        assert_eq!(element(&[0xfe, 0xff], 0, &field, 0, true).unwrap(), -2);
        assert_eq!(element(&[0xfe, 0xff], 0, &field, 0, false).unwrap(), 0xfffe);
    }

    #[test]
    fn uncovered_ranges_are_the_gaps_between_fields() {
        let fields = [field("a", 2, 0, 2, 1), field("b", 5, 0, 1, 1)];
        assert_eq!(uncovered(7, &fields), [[0, 2], [4, 5], [6, 7]]);
    }

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_790_467_200), "2026-09-27");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
    }

    #[test]
    fn the_progress_counter_is_not_compared() {
        assert!(fields().iter().all(|f| f.name != TRACKING_PROGRESS_FIELD));
        assert_eq!(fields().len(), CALIBRATION.len() + committed(0).len() - 1);
    }
}
