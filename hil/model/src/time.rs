//! One time axis for a run: an affine map from a source's clock (the
//! target, an air observer, a remote peer) to the host's monotonic clock,
//! with an error bound.
//!
//! Each source keeps its own time in the trace; the fit only maps it. A
//! target clock starts again at every boot, so a fit covers one boot (one
//! clock generation, #563 decision 6) and a new boot needs a new fit.

use core::fmt;

use serde::{Deserialize, Serialize};

/// One request/response pair: host send and receive on the host clock,
/// target receive and send on the target clock, all in microseconds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Exchange {
    pub host_send: u64,
    pub target_receive: u64,
    pub target_send: u64,
    pub host_receive: u64,
}

/// `host = offset + rate × (source − origin)`, valid within `error_micros`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TimeFit {
    pub origin: u64,
    pub offset: f64,
    pub rate: f64,
    pub error_micros: u64,
    pub exchanges: usize,
}

/// A source time on the host axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mapped {
    pub host_micros: u64,
    pub error_micros: u64,
}

/// Why a set of exchanges gives no fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FitError {
    NoExchange,
    /// An exchange whose target interval is not inside its host interval
    /// in order: a host reply received before it was sent, or a target
    /// reply sent before it was received.
    Disordered(usize),
    /// The clocks run at rates no crystal explains: a reboot inside the
    /// set, or two different clocks.
    Rate(f64),
}

impl fmt::Display for FitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoExchange => f.write_str("no exchange to fit"),
            Self::Disordered(index) => write!(f, "exchange {index} is out of order"),
            Self::Rate(rate) => write!(f, "rate {rate} is outside the crystal tolerance"),
        }
    }
}

impl std::error::Error for FitError {}

/// The largest rate deviation two crystal clocks show, in parts per
/// million: twice the ±50 ppm of an ordinary crystal, with margin for a
/// USB host clock.
const RATE_TOLERANCE_PPM: f64 = 500.0;

impl TimeFit {
    /// The least-squares fit through the exchanges' midpoints. The error
    /// bound covers every exchange: its residual plus half its round-trip
    /// delay, where the true offset may lie.
    pub fn from_exchanges(exchanges: &[Exchange]) -> Result<Self, FitError> {
        let first = exchanges.first().ok_or(FitError::NoExchange)?;
        let origin = first.target_receive;
        let mut points = Vec::with_capacity(exchanges.len());
        for (index, exchange) in exchanges.iter().enumerate() {
            if exchange.host_receive < exchange.host_send
                || exchange.target_send < exchange.target_receive
            {
                return Err(FitError::Disordered(index));
            }
            let source = (exchange.target_receive as f64 + exchange.target_send as f64) / 2.0
                - origin as f64;
            let host = (exchange.host_send as f64 + exchange.host_receive as f64) / 2.0;
            let round_trip = (exchange.host_receive - exchange.host_send) as f64
                - (exchange.target_send - exchange.target_receive) as f64;
            points.push((source, host, round_trip.max(0.0) / 2.0));
        }
        let count = points.len() as f64;
        let mean_source = points.iter().map(|p| p.0).sum::<f64>() / count;
        let mean_host = points.iter().map(|p| p.1).sum::<f64>() / count;
        let spread: f64 = points.iter().map(|p| (p.0 - mean_source).powi(2)).sum();
        let rate = if spread > 0.0 {
            points
                .iter()
                .map(|p| (p.0 - mean_source) * (p.1 - mean_host))
                .sum::<f64>()
                / spread
        } else {
            1.0
        };
        if ((rate - 1.0) * 1e6).abs() > RATE_TOLERANCE_PPM {
            return Err(FitError::Rate(rate));
        }
        let offset = mean_host - rate * mean_source;
        let error = points
            .iter()
            .map(|&(source, host, half)| (host - (offset + rate * source)).abs() + half)
            .fold(0.0_f64, f64::max);
        Ok(Self {
            origin,
            offset,
            rate,
            error_micros: error.ceil() as u64,
            exchanges: points.len(),
        })
    }

    /// A source time on the host axis.
    pub fn map(&self, source_micros: u64) -> Mapped {
        let host = self.offset + self.rate * (source_micros as f64 - self.origin as f64);
        Mapped {
            host_micros: host.max(0.0).round() as u64,
            error_micros: self.error_micros,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target clock 100 ppm fast and 5 s behind the host, with a link
    /// whose one-way delays are 300 µs out and 500 µs back.
    fn exchange(host_send: u64) -> Exchange {
        let target = |host: u64| ((host as f64 - 5_000_000.0) * 1.0001) as u64;
        let target_receive = target(host_send + 300);
        let target_send = target_receive + 50;
        let host_receive = host_send + 300 + 50 + 500;
        Exchange {
            host_send,
            target_receive,
            target_send,
            host_receive,
        }
    }

    #[test]
    fn the_fit_recovers_rate_and_offset_within_its_bound() {
        let exchanges: Vec<_> = (0..20)
            .map(|i| exchange(10_000_000 + i * 1_000_000))
            .collect();
        let fit = TimeFit::from_exchanges(&exchanges).unwrap();
        assert!((fit.rate - 1.0 / 1.0001).abs() < 1e-6, "{}", fit.rate);
        // An asymmetric link shifts the midpoint by (500 − 300) / 2 µs,
        // which the bound of half the round trip covers.
        let truth = 10_000_000 + 5 * 1_000_000 + 300;
        let mapped = fit.map(exchanges[5].target_receive);
        assert!(
            mapped.host_micros.abs_diff(truth) <= mapped.error_micros,
            "{mapped:?}"
        );
        assert!(mapped.error_micros < 1_000);
    }

    #[test]
    fn a_reboot_inside_the_set_is_refused() {
        let mut exchanges: Vec<_> = (0..4)
            .map(|i| exchange(10_000_000 + i * 1_000_000))
            .collect();
        for later in &mut exchanges[2..] {
            later.target_receive -= 3_000_000;
            later.target_send -= 3_000_000;
        }
        assert!(matches!(
            TimeFit::from_exchanges(&exchanges),
            Err(FitError::Rate(_))
        ));
    }

    #[test]
    fn disordered_or_missing_exchanges_are_refused() {
        assert_eq!(TimeFit::from_exchanges(&[]), Err(FitError::NoExchange));
        let mut backwards = exchange(1_000);
        backwards.host_receive = 0;
        assert_eq!(
            TimeFit::from_exchanges(&[backwards]),
            Err(FitError::Disordered(0))
        );
    }
}
