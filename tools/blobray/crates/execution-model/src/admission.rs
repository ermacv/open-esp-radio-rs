//! Suite-local admission at model call boundaries. Only denied mechanisms are
//! retained; this is not a dependency graph. Verification executes synchronously
//! on the owning thread and the guard cannot move to another thread.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    rc::Rc,
};

#[derive(Default)]
struct State {
    allowed: BTreeSet<String>,
    registers: BTreeMap<String, BTreeSet<String>>,
    denied: BTreeSet<String>,
}
thread_local! { static ACTIVE: RefCell<Option<State>> = const { RefCell::new(None) }; }

pub struct Scope(PhantomData<Rc<()>>);
impl Scope {
    pub fn enter(
        allowed: &[String],
        registers: BTreeMap<String, BTreeSet<String>>,
    ) -> crate::Result<Self> {
        ACTIVE.with_borrow_mut(|active| {
            if active.is_some() {
                return Err(crate::Error::invalid("nested model admission scope"));
            }
            *active = Some(State {
                allowed: allowed.iter().cloned().collect(),
                registers,
                ..State::default()
            });
            Ok(Self(PhantomData))
        })
    }
    pub fn check(&self) -> crate::Result<()> {
        ACTIVE.with_borrow(|active| {
            let denied = &active.as_ref().expect("active model scope").denied;
            if denied.is_empty() {
                Ok(())
            } else {
                Err(crate::Error::invalid(format!(
                    "undeclared model mechanisms: {denied:?}"
                )))
            }
        })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with_borrow_mut(|active| *active = None);
    }
}
pub fn active() -> bool {
    ACTIVE.with_borrow(|active| active.is_some())
}
pub fn mechanism(name: &str) {
    ACTIVE.with_borrow_mut(|active| {
        if let Some(state) = active
            && !state.allowed.contains(name)
        {
            state.denied.insert(name.into());
        }
    });
}
pub fn register(name: &str) {
    ACTIVE.with_borrow_mut(|active| {
        let Some(state) = active else {
            return;
        };
        match state.registers.get(name) {
            Some(owners) => state
                .denied
                .extend(owners.difference(&state.allowed).cloned()),
            None => {
                state.denied.insert(format!("unowned-register:{name}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undeclared_calls_and_registers_prevent_admission_without_leaking_between_suites() {
        let scope = Scope::enter(
            &["wifi".into()],
            BTreeMap::from([("BT.CONTROL".into(), BTreeSet::from(["bluetooth".into()]))]),
        )
        .unwrap();
        mechanism("wifi");
        scope.check().unwrap();
        register("BT.CONTROL");
        assert!(scope.check().is_err());
        drop(scope);
        let scope = Scope::enter(&["bluetooth".into()], BTreeMap::new()).unwrap();
        mechanism("bluetooth");
        scope.check().unwrap();
        mechanism("phy");
        assert!(scope.check().is_err());
    }
}
