//! Installed group owners and reserved record places. A pending installation
//! owns its vacant place, so its return cannot overflow a retirement pool.

use super::{GroupKeyId, Pool};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Index {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_selects_its_id_or_the_oldest_non_current_owner() {
        let mut keys = GroupKeys::new();
        let id = |id| GroupKeyId::new(id).unwrap();
        for value in 0..3 {
            let place = keys.reserve().unwrap();
            keys.install(
                Installed {
                    place,
                    key_id: id(value),
                    material: [value; 16],
                    key: value,
                },
                true,
            );
        }
        assert_eq!(keys.current_id(), Some(id(2)));
        let old = keys.replace(id(3), false).unwrap();
        assert_eq!(old.key, 0);
        assert_eq!(keys.current_id(), Some(id(2)));
        keys.install(
            Installed {
                place: old.place,
                key_id: id(3),
                material: [3; 16],
                key: 3,
            },
            true,
        );
        let same_id = keys.replace(id(2), false).unwrap();
        assert_eq!(same_id.key, 2);
        keys.install(same_id, false);
        assert_eq!(keys.take_retiring(), Some((id(1), 1)));
        assert_eq!(keys.take_retiring(), Some((id(2), 2)));
        assert_eq!(keys.replace(id(0), false).map(|entry| entry.key), None);
        let single = keys.replace(id(0), true).unwrap();
        assert_eq!(single.key, 3);
        keys.install(single, true);
        assert_eq!(keys.take_any(), Some((id(3), 3)));
        assert_eq!(keys.vacant.len(), 8);
    }
}

impl Index {
    const ALL: [Self; 8] = [
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
    ];

    fn select<T>(self, values: &[T; 8]) -> &T {
        let [a, b, c, d, e, f, g, h] = values;
        match self {
            Self::A => a,
            Self::B => b,
            Self::C => c,
            Self::D => d,
            Self::E => e,
            Self::F => f,
            Self::G => g,
            Self::H => h,
        }
    }

    fn select_mut<T>(self, values: &mut [T; 8]) -> &mut T {
        let [a, b, c, d, e, f, g, h] = values;
        match self {
            Self::A => a,
            Self::B => b,
            Self::C => c,
            Self::D => d,
            Self::E => e,
            Self::F => f,
            Self::G => g,
            Self::H => h,
        }
    }
}

/// One vacant record; minted privately once and moved through each install.
#[derive(Debug)]
pub(super) struct GroupPlace(Index);

pub(super) struct Installed<Key> {
    pub(super) place: GroupPlace,
    pub(super) key_id: GroupKeyId,
    pub(super) material: [u8; 16],
    pub(super) key: Key,
}

pub(super) struct GroupKeys<Key> {
    entries: [Option<Installed<Key>>; 8],
    vacant: Pool<GroupPlace>,
    /// Oldest first, without a wrapping or saturating installation counter.
    order: [Index; 8],
    current: Option<Index>,
}

impl<Key> GroupKeys<Key> {
    pub(super) fn new() -> Self {
        Self {
            entries: [const { None }; 8],
            vacant: Pool::new(Index::ALL.map(GroupPlace)),
            order: Index::ALL,
            current: None,
        }
    }

    pub(super) fn reserve(&mut self) -> Option<GroupPlace> {
        self.vacant.take()
    }

    pub(super) fn install(&mut self, entry: Installed<Key>, current: bool) {
        let index = entry.place.0;
        // The affine place proves this cell is empty. It can leave entries
        // only through take/replacement, both of which empty that same cell.
        *index.select_mut(&mut self.entries) = Some(entry);
        if current {
            self.current = Some(index);
        }
        let mut carry = index;
        for position in self.order.iter_mut().rev() {
            if *position == index {
                *position = carry;
                break;
            }
            core::mem::swap(position, &mut carry);
        }
    }

    pub(super) fn current_id(&self) -> Option<GroupKeyId> {
        self.current
            .and_then(|index| index.select(&self.entries).as_ref())
            .map(|entry| entry.key_id)
    }

    pub(super) fn same_material(&self, id: GroupKeyId, material: &[u8; 16]) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|entry| entry.key_id == id && entry.material == *material)
    }

    pub(super) fn has_retiring(&self) -> bool {
        self.oldest_retiring().is_some()
    }

    fn oldest_retiring(&self) -> Option<Index> {
        self.order
            .iter()
            .copied()
            .find(|index| Some(*index) != self.current && index.select(&self.entries).is_some())
    }

    pub(super) fn take_retiring(&mut self) -> Option<(GroupKeyId, Key)> {
        self.take(self.oldest_retiring()?)
    }

    pub(super) fn take_any(&mut self) -> Option<(GroupKeyId, Key)> {
        let index = self.oldest_retiring().or(self.current)?;
        self.take(index)
    }

    fn take(&mut self, index: Index) -> Option<(GroupKeyId, Key)> {
        let Installed {
            place, key_id, key, ..
        } = index.select_mut(&mut self.entries).take()?;
        if self.current == Some(index) {
            self.current = None;
        }
        // Only these eight private places can enter this pool. No installed
        // key owner is ever passed to Pool::put.
        self.vacant.put(place);
        Some((key_id, key))
    }

    /// Replace the same ID, otherwise the oldest non-current record. Only a
    /// backend with one physical slot may replace its sole current record.
    pub(super) fn replace(&mut self, id: GroupKeyId, single_slot: bool) -> Option<Installed<Key>> {
        let same_id = self.order.iter().rev().copied().find(|index| {
            index
                .select(&self.entries)
                .as_ref()
                .is_some_and(|entry| entry.key_id == id)
        });
        let index = same_id
            .or_else(|| self.oldest_retiring())
            .or_else(|| single_slot.then_some(self.current).flatten())?;
        index.select_mut(&mut self.entries).take()
    }
}
