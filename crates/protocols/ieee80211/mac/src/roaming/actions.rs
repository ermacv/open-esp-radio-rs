//! Assigned k/v Action categories and actions. The envelope here has a dialog
//! token; TFS Notify uses its own counted-ID format.
pub const WNM_CATEGORY: u8 = 10;
pub const RADIO_MEASUREMENT_CATEGORY: u8 = 5;
/// Category and Action octets precede the Dialog Token.
pub const DIALOG_TOKEN_OFFSET: usize = 2;
pub const ACTION_HEADER_LEN: usize = DIALOG_TOKEN_OFFSET + 1;
pub const AUTONOMOUS_DIALOG_TOKEN: u8 = 0;

/// IEEE 802.11-2012 Table 8-250. The action cannot be mixed with an RRM action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WnmAction {
    EventRequest = 0,
    EventReport = 1,
    DiagnosticRequest = 2,
    DiagnosticReport = 3,
    BssTransitionQuery = 6,
    BssTransitionRequest = 7,
    BssTransitionResponse = 8,
    TfsRequest = 13,
    TfsResponse = 14,
    TfsNotify = 15,
    WnmSleepRequest = 16,
    WnmSleepResponse = 17,
    DmsRequest = 23,
    DmsResponse = 24,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RrmAction {
    MeasurementRequest = 0,
    MeasurementReport = 1,
    LinkRequest = 2,
    LinkReport = 3,
    NeighborRequest = 4,
    NeighborResponse = 5,
}
