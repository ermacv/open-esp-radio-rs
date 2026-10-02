//! Checked compact ECC exchange. RustCrypto owns the curve arithmetic; OWE
//! owns only its compact representation, role ordering and key lifetime.
use super::*;
use p256::elliptic_curve::{
    group::Group as _,
    point::{AffineCoordinates, DecompactPoint},
};
use zeroize::Zeroizing;

enum Scalar {
    P256(p256::SecretKey),
    P384(p384::SecretKey),
    P521(p521::SecretKey),
}

/// One ephemeral private scalar. Inputs must be fresh uniform scalar octets
/// from the caller's entropy owner; zero/out-of-range values are rejected.
/// The scalar is never copied to a peer/cache and its crypto owner wipes it.
pub struct KeyPair {
    secret: Scalar,
    group: Group,
    public: [u8; crypto::MAX_COORDINATE_LEN],
}
impl KeyPair {
    pub fn from_scalar(group: Group, scalar: &[u8]) -> Result<Self, Error> {
        if scalar.len() != group.coordinate_len() {
            return Err(Error::InvalidKey);
        }
        let mut public = [0; crypto::MAX_COORDINATE_LEN];
        macro_rules! key {
            ($curve:ident, $variant:ident) => {{
                let key = $curve::SecretKey::from_slice(scalar).map_err(|_| Error::InvalidKey)?;
                let point = key.public_key().as_affine().x();
                public[..group.coordinate_len()].copy_from_slice(&point);
                Scalar::$variant(key)
            }};
        }
        let secret = match group {
            Group::P256 => key!(p256, P256),
            Group::P384 => key!(p384, P384),
            Group::P521 => key!(p521, P521),
        };
        Ok(Self {
            secret,
            group,
            public,
        })
    }
    pub const fn group(&self) -> Group {
        self.group
    }
    pub fn parameter(&self) -> oer_ieee80211_mac::owe::DhParameter<'_> {
        oer_ieee80211_mac::owe::DhParameter {
            group: self.group.number(),
            public_key: &self.public[..self.group.coordinate_len()],
        }
    }
    /// Consume the scalar after peer-point validation and non-infinite DH.
    /// Role determines C|A ordering; equal peer octets do not reorder roles.
    pub fn finish(
        self,
        role: crate::RsnInterface,
        addresses: Addresses,
        peer: oer_ieee80211_mac::owe::DhParameter<'_>,
    ) -> Result<OwePmk, Error> {
        addresses.validate()?;
        if peer.supported_group().map_err(Error::Wire)? != self.group {
            return Err(Error::WrongContext);
        }
        let mut shared = Zeroizing::new([0; crypto::MAX_COORDINATE_LEN]);
        macro_rules! exchange {
            ($curve:ident, $scalar:expr) => {{
                let mut x = $curve::FieldBytes::default();
                x.copy_from_slice(peer.public_key);
                let point = Option::<$curve::AffinePoint>::from($curve::AffinePoint::decompact(&x))
                    .ok_or(Error::InvalidKey)?;
                let scalar = Zeroizing::new($scalar.to_nonzero_scalar());
                let point = Zeroizing::new($curve::ProjectivePoint::from(point) * scalar.as_ref());
                if bool::from(point.is_identity()) {
                    return Err(Error::InvalidKey);
                }
                let affine = Zeroizing::new(point.to_affine());
                let mut x = affine.x();
                shared[..self.group.coordinate_len()].copy_from_slice(&x);
                zeroize::Zeroize::zeroize(&mut x);
            }};
        }
        match &self.secret {
            Scalar::P256(key) => exchange!(p256, key),
            Scalar::P384(key) => exchange!(p384, key),
            Scalar::P521(key) => exchange!(p521, key),
        }
        let own = &self.public[..self.group.coordinate_len()];
        let (station, ap) = match role {
            crate::RsnInterface::Station => (own, peer.public_key),
            crate::RsnInterface::AccessPoint => (peer.public_key, own),
        };
        OwePmk::derive(
            self.group,
            addresses,
            station,
            ap,
            &shared[..self.group.coordinate_len()],
        )
    }
}
