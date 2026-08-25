//! Reproducible, opt-in measurement of the production inline/CAS boundary.

use std::{fs, hint::black_box, path::PathBuf, time::Instant};

use serde::Serialize;

use super::{INLINE_VALUE_LIMIT, QueryStore};

const RECORDS_PER_SIZE: usize = 16;
const PAYLOAD_SIZES: [usize; 4] = [
    16 * 1024,
    INLINE_VALUE_LIMIT,
    INLINE_VALUE_LIMIT + 1,
    256 * 1024,
];

#[derive(Serialize)]
struct PolicyMeasurement {
    schema_version: u32,
    os: &'static str,
    architecture: &'static str,
    profile: &'static str,
    root_source: &'static str,
    inline_limit_bytes: usize,
    records_per_size: usize,
    samples: Vec<PolicySample>,
}

#[derive(Serialize)]
struct PolicySample {
    payload_bytes: usize,
    storage: &'static str,
    write_microseconds: u128,
    read_microseconds: u128,
    database_bytes: u64,
    pack_bytes: u64,
    cache_root_bytes: u64,
}

/// Run with:
///
/// `BLOBRAY_CACHE_BENCH_ROOT=target BLOBRAY_CACHE_BENCH_OUTPUT=target/cache-policy-measurement.json cargo test -p blobray cache_storage_policy_measurement --release -- --ignored --test-threads=1`
///
/// Timings are diagnostic, not pass/fail thresholds. The assertions ensure
/// that the measurement continues to exercise the production storage policy.
#[test]
#[ignore = "opt-in storage-policy diagnostic writes and fsyncs multiple fresh caches"]
fn cache_storage_policy_measurement() {
    let configured_root = std::env::var_os("BLOBRAY_CACHE_BENCH_ROOT").map(PathBuf::from);
    let root_source = if configured_root.is_some() {
        "BLOBRAY_CACHE_BENCH_ROOT"
    } else {
        "temp-dir"
    };
    let root = configured_root
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "blobray-cache-policy-measurement-{}",
            std::process::id()
        ));
    if root.is_dir() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let mut samples = Vec::new();

    for payload_bytes in PAYLOAD_SIZES {
        let manifest = measurement_manifest(&root, payload_bytes);
        let mut store = QueryStore::open(&manifest).unwrap();
        let started = Instant::now();
        let mut keys = Vec::with_capacity(RECORDS_PER_SIZE);
        for index in 0..RECORDS_PER_SIZE {
            let key = format!("policy:{payload_bytes}:{index}");
            let mut value = vec![0x5a; payload_bytes];
            value[..size_of::<u64>()].copy_from_slice(&(index as u64).to_le_bytes());
            store
                .put(&key, "storage-policy-measurement", &key, &[], &value)
                .unwrap();
            keys.push((key, value));
        }
        let write_microseconds = started.elapsed().as_micros();
        let (inline_rows, packed_rows) = store
            .connection
            .query_row(
                "SELECT COUNT(inline_value), COUNT(object_digest)
                 FROM query_results WHERE kind = 'storage-policy-measurement'",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        let expected_inline = payload_bytes <= INLINE_VALUE_LIMIT;
        assert_eq!(
            inline_rows,
            if expected_inline {
                RECORDS_PER_SIZE as i64
            } else {
                0
            }
        );
        assert_eq!(
            packed_rows,
            if expected_inline {
                0
            } else {
                RECORDS_PER_SIZE as i64
            }
        );
        drop(store);

        let statistics = QueryStore::statistics(&manifest).unwrap();
        let store = QueryStore::open(&manifest).unwrap();
        let started = Instant::now();
        for (key, expected) in keys {
            let value = store.get(&key).unwrap().unwrap();
            assert_eq!(black_box(value), expected);
        }
        let read_microseconds = started.elapsed().as_micros();
        drop(store);
        samples.push(PolicySample {
            payload_bytes,
            storage: if expected_inline {
                "sqlite-inline"
            } else {
                "cas-pack"
            },
            write_microseconds,
            read_microseconds,
            database_bytes: statistics.database_bytes,
            pack_bytes: statistics.pack_bytes,
            cache_root_bytes: statistics.root_bytes,
        });
    }

    let measurement = PolicyMeasurement {
        schema_version: 1,
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        root_source,
        inline_limit_bytes: INLINE_VALUE_LIMIT,
        records_per_size: RECORDS_PER_SIZE,
        samples,
    };
    if let Some(output) = std::env::var_os("BLOBRAY_CACHE_BENCH_OUTPUT") {
        let output = PathBuf::from(output);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(output, serde_json::to_vec_pretty(&measurement).unwrap()).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

fn measurement_manifest(root: &std::path::Path, payload_bytes: usize) -> PathBuf {
    let project = root.join(payload_bytes.to_string());
    fs::create_dir_all(&project).unwrap();
    project.join("vendor-project.toml")
}
