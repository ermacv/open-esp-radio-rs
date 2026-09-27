extern crate std;

use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    StartRead(u8),
    StartWrite(u8, u8),
}

/// One-host register model whose commands stay busy for `latency` polls.
struct Model {
    registers: [u8; 4],
    latency: u8,
    busy: u8,
    operations: Vec<Operation>,
}

impl Model {
    fn new(latency: u8) -> Self {
        Self {
            registers: [0; 4],
            latency,
            busy: 0,
            operations: Vec::new(),
        }
    }

    fn tick(&mut self) {
        self.busy = self.busy.saturating_sub(1);
    }
}

impl AnalogRegisterBus for Model {
    type Address = u8;

    fn try_start_read(&mut self, address: u8) -> Result<(), Busy> {
        if self.busy != 0 {
            return Err(Busy);
        }
        self.busy = self.latency;
        self.operations.push(Operation::StartRead(address));
        Ok(())
    }

    fn try_finish_read(&self, address: u8) -> Result<u8, Busy> {
        if self.busy != 0 {
            Err(Busy)
        } else {
            Ok(self.registers[usize::from(address)])
        }
    }

    fn try_start_write(&mut self, address: u8, value: u8) -> Result<(), Busy> {
        if self.busy != 0 {
            return Err(Busy);
        }
        self.busy = self.latency;
        self.registers[usize::from(address)] = value;
        self.operations.push(Operation::StartWrite(address, value));
        Ok(())
    }

    fn try_finish_write(&self, _address: u8) -> Result<(), Busy> {
        if self.busy != 0 { Err(Busy) } else { Ok(()) }
    }
}

fn run<T>(model: &mut Model, mut poll: impl FnMut(&mut Model) -> Step<T>) -> T {
    for _ in 0..64 {
        if let Step::Ready(value) = poll(model) {
            return value;
        }
        model.tick();
    }
    panic!("the transaction did not complete");
}

#[test]
fn fields_are_bounded_to_one_byte() {
    assert!(AnalogField::new(0_u8, 7, 0).is_some());
    assert!(AnalogField::new(0_u8, 3, 4).is_none());
    assert!(AnalogField::new(0_u8, 8, 0).is_none());
}

#[test]
fn extraction_and_insertion_follow_the_vendor_leaves() {
    let field = AnalogField::new(0_u8, 5, 2).expect("field");
    assert_eq!(field.extract(0b1011_0110), 0b1101);
    assert_eq!(field.insert(0b1111_1111, 0b0010), Some(0b1100_1011));
    // A value wider than the field would leak above it in the vendor leaf.
    assert_eq!(field.insert(0, 0b1_0000), None);
    let whole = AnalogField::new(0_u8, 7, 0).expect("field");
    assert_eq!(whole.insert(0x12, 0xab), Some(0xab));
}

#[test]
fn a_field_read_extracts_after_the_completed_read() {
    let mut model = Model::new(3);
    model.registers[2] = 0b0110_0000;
    let mut read = FieldRead::new(AnalogField::new(2_u8, 6, 5).expect("field"));
    assert_eq!(run(&mut model, |bus| read.poll(bus)), 0b11);
    assert_eq!(model.operations, [Operation::StartRead(2)]);
}

#[test]
fn a_field_write_reads_modifies_and_writes_once() {
    let mut model = Model::new(2);
    model.registers[1] = 0b1010_1010;
    let mut write =
        FieldWrite::new(AnalogField::new(1_u8, 3, 0).expect("field"), 0b0101).expect("fits");
    run(&mut model, |bus| write.poll(bus));
    assert_eq!(
        model.operations,
        [
            Operation::StartRead(1),
            Operation::StartWrite(1, 0b1010_0101)
        ]
    );
}

#[test]
fn a_busy_host_delays_the_start_without_losing_the_command() {
    let mut model = Model::new(1);
    model.busy = 5;
    let mut write = FieldWrite::new(AnalogField::new(0_u8, 0, 0).expect("field"), 1).expect("fits");
    run(&mut model, |bus| write.poll(bus));
    assert_eq!(
        model.operations,
        [Operation::StartRead(0), Operation::StartWrite(0, 1)]
    );
}

#[test]
fn a_value_wider_than_its_field_is_rejected() {
    let field = AnalogField::new(0_u8, 1, 0).expect("field");
    assert_eq!(FieldWrite::new(field, 4).err(), Some(ValueTooWide));
}
