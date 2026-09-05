//! Executor bindings for the public radio control contracts.
//!
//! Runtime modules transport commands and drive local role epochs; they do not
//! acquire hardware independently of the concrete integration.

pub mod embassy;
