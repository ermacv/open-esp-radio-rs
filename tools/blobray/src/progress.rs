//! Frontend-neutral construction helpers for determinate progress spans.

use tracing::Span;
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Start a span whose total work is known and whose position is advanced by its caller.
pub(crate) fn determinate_span(span: Span, length: usize, message: &str) -> Span {
    span.pb_set_length(length.try_into().unwrap_or(u64::MAX));
    span.pb_set_message(message);
    span.pb_start();
    span
}
