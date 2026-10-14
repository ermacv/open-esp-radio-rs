//! Address roles validated before encoding an individual station frame.

/// The receiver of an individual frame: its address has the group bit
/// clear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualAddress([u8; 6]);

impl IndividualAddress {
    pub(crate) const ZERO: Self = Self([0; 6]);

    /// `address`, when its group bit is clear.
    pub const fn new(address: [u8; 6]) -> Option<Self> {
        if address[0] & 1 == 0 {
            Some(Self(address))
        } else {
            None
        }
    }

    /// The address.
    pub const fn get(self) -> [u8; 6] {
        self.0
    }
}

/// The individual address of the local interface.
/// This role cannot be substituted for the other address in a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnAddress(IndividualAddress);

impl OwnAddress {
    /// Assign the role to a validated individual address.
    pub const fn new(address: IndividualAddress) -> Self {
        Self(address)
    }

    /// The individual address with its group-bit proof.
    pub const fn individual(self) -> IndividualAddress {
        self.0
    }

    /// The wire octets.
    pub const fn get(self) -> [u8; 6] {
        self.0.get()
    }
}

/// The individual address identifying an infrastructure BSS.
/// This role cannot be substituted for the other address in a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bssid(IndividualAddress);

impl Bssid {
    /// Assign the role to a validated individual address.
    pub const fn new(address: IndividualAddress) -> Self {
        Self(address)
    }

    /// The individual address with its group-bit proof.
    pub const fn individual(self) -> IndividualAddress {
        self.0
    }

    /// The wire octets.
    pub const fn get(self) -> [u8; 6] {
        self.0.get()
    }
}
