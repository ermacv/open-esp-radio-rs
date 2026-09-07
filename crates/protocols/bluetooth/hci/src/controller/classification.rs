//! Portable command classification before chip-specific session execution.

use bt_hci::cmd::Opcode;

use super::bootstrap::{BootstrapCommandDecodeError, invalid_parameters};

use crate::{
    BootstrapCommandCompleteEvent, HciCommandPacket, HciEpochBound, LeDtmCommand,
    LeDtmCommandCompleteEvent, LeLegacyAdvertisingCommand, LeLegacyAdvertisingCommandCompleteEvent,
    LeLegacyAdvertisingCommandKind, LeLegacyAdvertisingConfigurationCommand,
    LeLegacyAdvertisingEnableCommand, LeLegacyScanningCommand,
    LeLegacyScanningCommandCompleteEvent, LeLegacyScanningCommandKind,
    LeLegacyScanningConfigurationCommand, LeLegacyScanningEnableCommand, LeTestEndCommand,
    OwnedBootstrapCommand, UnknownCommandCompleteEvent,
};

/// One finite result of classifying a validated HCI command packet.
///
/// Bootstrap, DTM and software-only role configuration commands are
/// owned semantic tokens. Malformed responses are complete owned packets that
/// may be retained across Controller-to-Host backpressure. An opcode outside
/// the closed table becomes an owned Unknown Command response, so no
/// classification result borrows receive storage.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a classified HCI command must be routed or answered exactly once"]
pub enum LeControllerCommandClassification {
    /// A decoded bootstrap command awaits session-aware software dispatch.
    Bootstrap(OwnedBootstrapCommand),
    /// A known bootstrap opcode had malformed parameters and produced this response.
    MalformedBootstrap(BootstrapCommandCompleteEvent),
    /// A validated DTM command awaits chip-specific session execution.
    Dtm(LeDtmCommand),
    /// A known DTM opcode had malformed parameters and produced this response.
    MalformedDtm(LeDtmCommandCompleteEvent),
    /// A validated software-only legacy advertising configuration command.
    LegacyAdvertisingConfiguration(LeLegacyAdvertisingConfigurationCommand),
    /// A validated Enable command awaits radio-lifecycle policy.
    LegacyAdvertisingEnable(LeLegacyAdvertisingEnableCommand),
    /// A claimed legacy advertising opcode was malformed.
    MalformedLegacyAdvertising(LeLegacyAdvertisingCommandCompleteEvent),
    /// A validated software-only legacy scanning configuration command.
    LegacyScanningConfiguration(LeLegacyScanningConfigurationCommand),
    /// A validated Set Scan Enable command awaits radio-lifecycle policy.
    LegacyScanningEnable(LeLegacyScanningEnableCommand),
    /// A claimed legacy scanning opcode was malformed or unsupported.
    MalformedLegacyScanning(LeLegacyScanningCommandCompleteEvent),
    /// This closed Controller command table did not claim the opcode.
    Unsupported(UnknownCommandCompleteEvent),
}

impl LeControllerCommandClassification {
    /// Opcode retained by this exact classification result.
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Bootstrap(command) => command.opcode(),
            Self::MalformedBootstrap(response) => response.opcode(),
            Self::Dtm(command) => command.kind().opcode(),
            Self::MalformedDtm(response) => response.opcode(),
            Self::LegacyAdvertisingConfiguration(command) => command.kind().opcode(),
            Self::LegacyAdvertisingEnable(_) => LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
            Self::MalformedLegacyAdvertising(response) => response.opcode(),
            Self::LegacyScanningConfiguration(_) => {
                LeLegacyScanningCommandKind::SetParameters.opcode()
            }
            Self::LegacyScanningEnable(_) => LeLegacyScanningCommandKind::SetEnable.opcode(),
            Self::MalformedLegacyScanning(response) => response.opcode(),
            Self::Unsupported(response) => response.opcode(),
        }
    }
}

impl<'epoch> HciEpochBound<'epoch, LeControllerCommandClassification> {
    /// Refine an epoch-bound production classification into a semantic DTM command.
    pub fn try_into_dtm(self) -> Result<HciEpochBound<'epoch, LeDtmCommand>, Self> {
        self.try_map(|classification| match classification {
            LeControllerCommandClassification::Dtm(command) => Ok(command),
            classification => Err(classification),
        })
    }
}

impl<'epoch> HciEpochBound<'epoch, LeDtmCommand> {
    /// Refine an epoch-bound DTM command into the semantic Test End owner.
    pub fn try_into_test_end(self) -> Result<HciEpochBound<'epoch, LeTestEndCommand>, Self> {
        self.try_map(|command| match command {
            LeDtmCommand::TestEnd(command) => Ok(command),
            command => Err(command),
        })
    }
}

/// Classify one validated command without executing radio or Link-Layer work.
///
/// DTM decoding runs first because it distinguishes malformed input for its
/// known opcodes from an opcode outside that command family. The latter
/// is decoded only when it belongs to the closed bootstrap table; otherwise
/// classification builds its terminal owned Unknown Command response.
/// Classification never observes or advances a bootstrap epoch. No branch
/// waits, publishes a response, or claims hardware readiness.
pub fn classify_le_controller_command(
    command: HciCommandPacket<'_>,
) -> LeControllerCommandClassification {
    if LeLegacyAdvertisingCommandKind::from_opcode(command.opcode()).is_some() {
        return match LeLegacyAdvertisingCommand::decode(command) {
            Ok(command) => match LeLegacyAdvertisingConfigurationCommand::from_command(command) {
                Ok(command) => {
                    LeControllerCommandClassification::LegacyAdvertisingConfiguration(command)
                }
                Err(command) => LeControllerCommandClassification::LegacyAdvertisingEnable(
                    LeLegacyAdvertisingEnableCommand::from_command(command)
                        .expect("the non-configuration advertising command is Enable"),
                ),
            },
            Err(error) => LeControllerCommandClassification::MalformedLegacyAdvertising(
                error
                    .into_command_complete()
                    .expect("a claimed advertising opcode must build an exact completion"),
            ),
        };
    }

    if LeLegacyScanningCommandKind::from_opcode(command.opcode()).is_some() {
        return match LeLegacyScanningCommand::decode(command) {
            Ok(command) => match LeLegacyScanningConfigurationCommand::from_command(command) {
                Ok(command) => {
                    LeControllerCommandClassification::LegacyScanningConfiguration(command)
                }
                Err(LeLegacyScanningCommand::SetEnable(command)) => {
                    LeControllerCommandClassification::LegacyScanningEnable(command)
                }
                Err(LeLegacyScanningCommand::SetParameters(_)) => {
                    unreachable!("Set Parameters refines into scanning configuration")
                }
            },
            Err(error) => LeControllerCommandClassification::MalformedLegacyScanning(
                error
                    .into_command_complete()
                    .expect("a claimed scanning opcode must build an exact completion"),
            ),
        };
    }

    match LeDtmCommand::decode(command) {
        Ok(command) => LeControllerCommandClassification::Dtm(command),
        Err(error) => match error.into_command_complete() {
            Ok(response) => LeControllerCommandClassification::MalformedDtm(response),
            Err(_) => match OwnedBootstrapCommand::decode(command) {
                Ok(command) => LeControllerCommandClassification::Bootstrap(command),
                Err(BootstrapCommandDecodeError::Malformed(kind)) => {
                    LeControllerCommandClassification::MalformedBootstrap(invalid_parameters(
                        kind.opcode(),
                    ))
                }
                Err(BootstrapCommandDecodeError::Unsupported) => {
                    LeControllerCommandClassification::Unsupported(
                        UnknownCommandCompleteEvent::new(command.opcode()),
                    )
                }
            },
        },
    }
}

#[cfg(test)]
mod tests;
