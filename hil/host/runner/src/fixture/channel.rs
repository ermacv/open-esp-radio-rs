//! Active channel geometry shared by AP verification and passive observers.
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Geometry {
    pub(crate) frequency: u16,
    pub(crate) width: u16,
    pub(crate) center: u16,
}

impl Geometry {
    pub(crate) fn parse(info: &str) -> Result<Self> {
        let line = info
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("channel "))
            .ok_or("interface did not report a channel")?;
        let frequency = line
            .split_once('(')
            .and_then(|(_, tail)| tail.split_whitespace().next())
            .ok_or("interface omitted primary frequency")?
            .parse()?;
        let value = |key: &str| -> Result<u16> {
            Ok(line
                .split_once(key)
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .ok_or("interface omitted channel geometry")?
                .parse()?)
        };
        let geometry = Self {
            frequency,
            width: value("width:")?,
            center: value("center1:")?,
        };
        geometry.iw_width()?;
        Ok(geometry)
    }

    pub(crate) fn iw_width(self) -> Result<&'static str> {
        match (
            self.width,
            i32::from(self.center) - i32::from(self.frequency),
        ) {
            (20, 0) => Ok("HT20"),
            (40, 10) => Ok("HT40+"),
            (40, -10) => Ok("HT40-"),
            _ => Err("unsupported or inconsistent monitor channel geometry".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observer_retains_twenty_mhz_and_both_secondary_directions() {
        for (line, width) in [
            (
                "channel 13 (2472 MHz), width: 20 MHz, center1: 2472 MHz",
                "HT20",
            ),
            (
                "channel 3 (2422 MHz), width: 40 MHz, center1: 2432 MHz",
                "HT40+",
            ),
            (
                "channel 6 (2437 MHz), width: 40 MHz, center1: 2427 MHz",
                "HT40-",
            ),
        ] {
            assert_eq!(Geometry::parse(line).unwrap().iw_width().unwrap(), width);
        }
    }
}
