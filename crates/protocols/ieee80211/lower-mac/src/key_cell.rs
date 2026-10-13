//! A station's affine key slots retain their cell through install and removal.

/// A closed selector for the station's at most eight group-key cells.
/// It selects by construction, without comparing hardware key identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyCellIndex {
    #[doc(hidden)]
    C0,
    #[doc(hidden)]
    C1,
    #[doc(hidden)]
    C2,
    #[doc(hidden)]
    C3,
    #[doc(hidden)]
    C4,
    #[doc(hidden)]
    C5,
    #[doc(hidden)]
    C6,
    #[doc(hidden)]
    C7,
}

impl KeyCellIndex {
    /// Borrow this cell. Each arm indexes a fixed array at a constant offset.
    pub fn select<T>(self, cells: &mut [T; 8]) -> &mut T {
        match self {
            Self::C0 => &mut cells[0],
            Self::C1 => &mut cells[1],
            Self::C2 => &mut cells[2],
            Self::C3 => &mut cells[3],
            Self::C4 => &mut cells[4],
            Self::C5 => &mut cells[5],
            Self::C6 => &mut cells[6],
            Self::C7 => &mut cells[7],
        }
    }
}

/// The returned slot itself identifies the cell it frees.
pub trait KeyCellResource: private::Sealed {
    /// The slot's cell, retained across installation and removal.
    fn cell(&self) -> KeyCellIndex;
}

mod private {
    pub trait Sealed {}
}

/// A unique key slot bound to one station cell. Backends keep this wrapper
/// inside the installed key and return it intact after the removal barrier.
/// There is deliberately no consuming conversion back to the unbound slot.
#[derive(Debug, Eq, PartialEq)]
pub struct KeyCellSlot<S> {
    slot: S,
    cell: KeyCellIndex,
}

impl<S> KeyCellSlot<S> {
    /// Bind the installation's group-slot pool once, before granting it to
    /// Station. The backend must not mint a second pool for that station.
    ///
    /// ```compile_fail,E0080
    /// use oer_ieee80211_lower_mac::KeyCellSlot;
    /// let slots = KeyCellSlot::bind([(); 9]);
    /// core::hint::black_box(slots);
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_lower_mac::{KeyCellResource, KeyCellSlot};
    /// let slots = KeyCellSlot::bind([(); 8]);
    /// let mut cells = [false; 8];
    /// for slot in slots { *slot.cell().select(&mut cells) = true; }
    /// assert_eq!(cells, [true; 8]);
    /// ```
    pub fn bind<const N: usize>(slots: [S; N]) -> [Self; N] {
        const { check_capacity(N) };
        use KeyCellIndex::{C0, C1, C2, C3, C4, C5, C6, C7};
        let mut cells = [C0, C1, C2, C3, C4, C5, C6, C7];
        slots.map(|slot| {
            let [cell, b, c, d, e, f, g, h] = cells;
            cells = [b, c, d, e, f, g, h, cell];
            Self { slot, cell }
        })
    }

    /// Borrow the physical slot without discarding its cell ownership.
    pub fn slot(&self) -> &S {
        &self.slot
    }
}

impl<S> private::Sealed for KeyCellSlot<S> {}
impl<S> KeyCellResource for KeyCellSlot<S> {
    fn cell(&self) -> KeyCellIndex {
        self.cell
    }
}

#[expect(clippy::panic, reason = "evaluated at build")]
const fn check_capacity(n: usize) {
    if n > 8 {
        panic!("a station has at most eight group-key cells");
    }
}
