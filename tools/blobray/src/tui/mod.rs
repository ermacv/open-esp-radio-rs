//! Read-only project browser built on the public application facade.

mod interface_rows;
mod runtime;
mod state;
mod view;
mod worker;

pub(crate) use runtime::run;

#[cfg(test)]
fn register_fixture(address: u64, name: &str) -> crate::InventoryRegister {
    use open_radio_vendor_contracts::register_inventory::{KnowledgeProperty, RegisterSubject};
    let subject = RegisterSubject {
        chip: "fixture".to_owned(),
        address_space: "cpu".to_owned(),
        route: "mmio".to_owned(),
        bank: None,
        address,
    };
    let mut names = KnowledgeProperty::Unknown;
    names.insert(name.to_owned(), "fixture-name".to_owned());
    crate::InventoryRegister {
        id: subject.id(),
        subject,
        names,
        physical_width: KnowledgeProperty::Unknown,
        semantics: KnowledgeProperty::Unknown,
        access_widths: Default::default(),
        functions: Default::default(),
        fields: Default::default(),
        evidence: Default::default(),
        coverage: Default::default(),
    }
}

#[cfg(test)]
fn register_snapshot(inventory: crate::RegisterInventory) -> crate::RegisterInventoryState {
    crate::RegisterInventoryState::Available {
        snapshot: std::sync::Arc::new(crate::RegisterInventorySnapshot::new(inventory).unwrap()),
    }
}

#[cfg(test)]
fn replace_registers(
    report: &mut crate::RegisterWorkspaceReport,
    registers: Vec<crate::InventoryRegister>,
) {
    let mut inventory = report
        .inventory
        .snapshot()
        .map(|s| s.inventory().clone())
        .unwrap_or_default();
    inventory.registers = registers.into_iter().map(|r| (r.id.clone(), r)).collect();
    report.inventory = register_snapshot(inventory);
}

#[cfg(test)]
fn append_register(
    report: &mut crate::RegisterWorkspaceReport,
    register: crate::InventoryRegister,
) {
    let mut registers = report.registers().cloned().collect::<Vec<_>>();
    registers.push(register);
    replace_registers(report, registers);
}
