//! Rust rendering for validated safe PAC transaction declarations.

use std::collections::BTreeSet;

use svd_rs::Device;

use crate::{PacApiPack, Result, pac_api_svd};

impl PacApiPack {
    /// Render helper modules to append to the svd2rust crate root.
    pub fn render_rust(&self, svd: &str) -> Result<String> {
        self.validate_against_svd(svd)?;
        let device = svd_parser::parse(svd)?;
        let mut output = String::new();
        output.push_str(&self.render_interrupt_snapshots());
        if self.options.peripheral_ownership {
            output.push_str(&self.render_peripheral_ownership(&device)?);
        }
        output.push_str(&self.render_full_register_writes());
        output.push_str(&self.render_fixed_register_writes());
        output.push_str(&self.render_fixed_register_images(&device)?);
        output.push_str(&self.render_register_image_writes(&device)?);
        output.push_str(&self.render_zero_based_field_writes(&device)?);
        output.push_str(&self.render_zero_register_writes(&device)?);
        output.push_str(&self.render_masked_register_modifies(&device)?);
        if self.options.device_access {
            output.push_str(device_access_api());
        }
        Ok(output)
    }

    fn render_interrupt_snapshots(&self) -> String {
        if self.interrupt_snapshots.is_empty() {
            return String::new();
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared read-and-acknowledge interrupt transactions.\n\
             pub mod interrupt_snapshot {\n",
        );
        for binding in &self.interrupt_snapshots {
            let snapshot_type = format!("{}Snapshot", type_binding_name(&binding.name));
            let peripheral_type = type_binding_name(&binding.peripheral);
            let status = member_binding_name(&binding.status_register);
            let clear = member_binding_name(&binding.clear_register);
            let clear_field = member_binding_name(&binding.clear_field);
            output.push_str(&format!(
                "\n    /// Opaque event image sampled from `{}`.`{}`.\n\
                 #[must_use = \"an interrupt snapshot must be inspected and acknowledged\"]\n\
                 #[derive(Debug)]\n\
                 pub struct {snapshot_type}(u32);\n\
                 impl {snapshot_type} {{\n\
                     /// Complete masked event image observed by the status read.\n\
                     #[inline]\n\
                     pub const fn bits(&self) -> u32 {{ self.0 }}\n\
                 }}\n\
                 /// Sample the complete masked event image.\n\
                 #[inline]\n\
                 pub fn sample_{}(registers: &crate::{peripheral_type}) -> {snapshot_type} {{\n\
                     {snapshot_type}(registers.{status}().read().bits())\n\
                 }}\n\
                 /// Acknowledge exactly the event image returned by the paired sample.\n\
                 #[inline]\n\
                 pub fn acknowledge_{}(\n\
                     registers: &crate::{peripheral_type},\n\
                     snapshot: {snapshot_type},\n\
                 ) {{\n\
                     // SAFETY: the opaque value can only be constructed by the paired\n\
                     // STATUS read (or in a validation-only build) and CLEAR is an\n\
                     // SVD-validated full-width write-one-to-clear register.\n\
                     unsafe {{\n\
                         registers.{clear}().write_with_zero(|writer|\n\
                             writer.{clear_field}().bits(snapshot.0)\n\
                         );\n\
                     }}\n\
                 }}\n\
                 #[cfg(feature = \"validation-probes\")]\n\
                 #[doc(hidden)]\n\
                 pub const fn {}_for_validation(bits: u32) -> {snapshot_type} {{\n\
                     {snapshot_type}(bits)\n\
                 }}\n",
                binding.peripheral,
                binding.status_register,
                binding.name,
                binding.name,
                binding.name,
            ));
        }
        output.push_str("}\n");
        output
    }

    fn render_peripheral_ownership(&self, device: &Device) -> Result<String> {
        let interrupt_names = self
            .interrupt_snapshots
            .iter()
            .map(|binding| binding.peripheral.as_str())
            .collect::<BTreeSet<_>>();
        let peripheral_names = device
            .peripherals
            .iter()
            .map(|peripheral| peripheral.name.clone())
            .collect::<Vec<_>>();
        let ordinary_peripherals = peripheral_names
            .iter()
            .filter(|name| !interrupt_names.contains(name.as_str()))
            .collect::<Vec<_>>();
        let interrupt_peripherals = peripheral_names
            .iter()
            .filter(|name| interrupt_names.contains(name.as_str()))
            .collect::<Vec<_>>();
        if interrupt_peripherals.len() != interrupt_names.len() {
            return Err("PAC API interrupt ownership references an unknown peripheral".into());
        }
        let fields = |names: &[&String]| {
            names
                .iter()
                .map(|name| {
                    format!(
                        "    pub {}: crate::{},\n",
                        member_binding_name(name),
                        type_binding_name(name),
                    )
                })
                .collect::<String>()
        };
        let members = |names: &[&String]| {
            names
                .iter()
                .map(|name| format!("        {},\n", member_binding_name(name)))
                .collect::<String>()
        };
        let all_members = members(&peripheral_names.iter().collect::<Vec<_>>());
        let ordinary_members = members(&ordinary_peripherals);
        let interrupt_members = members(&interrupt_peripherals);
        Ok(format!(
            "\n/// Safe ownership partitions derived from the SVD interrupt banks.\n\
             pub mod peripheral_ownership {{\n\
             /// Radio peripherals which remain available to ordinary task code.\n\
             #[allow(non_snake_case)]\n\
             pub struct RadioPeripherals {{\n{}         }}\n\
             /// Interrupt banks transferred from cold setup to the hard handlers.\n\
             #[allow(non_snake_case)]\n\
             pub struct InterruptPeripherals {{\n{}         }}\n\
             /// Consume the singleton and separate task-owned registers from interrupt banks.\n\
             #[inline]\n\
             pub fn split(\n\
                 peripherals: crate::Peripherals,\n\
             ) -> (RadioPeripherals, InterruptPeripherals) {{\n\
                 let crate::Peripherals {{\n{}             }} = peripherals;\n\
                 (\n\
                     RadioPeripherals {{\n{}                 }},\n\
                     InterruptPeripherals {{\n{}                 }},\n\
                 )\n\
             }}\n\
             /// Acquire a fresh singleton in an isolated compiled-validation image.\n\
             #[cfg(feature = \"validation-probes\")]\n\
             #[doc(hidden)]\n\
             #[inline]\n\
             pub fn peripherals_for_validation() -> crate::Peripherals {{\n\
                 // SAFETY: validation images contain one closed probe and no runtime driver.\n\
                 unsafe {{ crate::Peripherals::steal() }}\n\
             }}\n\
             }}\n",
            fields(&ordinary_peripherals),
            fields(&interrupt_peripherals),
            all_members,
            ordinary_members,
            interrupt_members,
        ))
    }

    fn render_full_register_writes(&self) -> String {
        if self.full_register_writes.is_empty() {
            return String::new();
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared writes which cover a complete register.\n\
             pub mod full_register_write {\n",
        );
        for binding in &self.full_register_writes {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let field = member_binding_name(&binding.field);
            output.push_str(&format!(
                "\n    /// Write every bit of `{}`.`{}` through its full-width field.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}, value: u32) {{\n\
                     // SAFETY: generator validation proves that this is the only field,\n\
                     // it covers all 32 bits and accepts every `u32`; no zero-filled\n\
                     // reserved or partially described bits remain.\n\
                     unsafe {{\n\
                         registers.{register}().write_with_zero(|writer|\n\
                             writer.{field}().set(value)\n\
                         );\n\
                     }}\n\
                 }}\n",
                binding.peripheral, binding.register, binding.name,
            ));
        }
        output.push_str("}\n");
        output
    }

    fn render_fixed_register_writes(&self) -> String {
        if self.fixed_register_writes.is_empty() {
            return String::new();
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared complete-register writes of fixed enumerated values.\n\
             pub mod fixed_register_write {\n",
        );
        for binding in &self.fixed_register_writes {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let field = member_binding_name(&binding.field);
            let variant = member_binding_name(&binding.variant);
            output.push_str(&format!(
                "\n    /// Write the `{}` variant to every bit of `{}`.`{}`.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}) {{\n\
                     // SAFETY: generator validation proves that the sole field covers\n\
                     // all 32 bits and the named writable variant exists in the SVD.\n\
                     unsafe {{\n\
                         registers.{register}().write_with_zero(|writer|\n\
                             writer.{field}().{variant}()\n\
                         );\n\
                     }}\n\
                 }}\n",
                binding.variant, binding.peripheral, binding.register, binding.name,
            ));
        }
        output.push_str("}\n");
        output
    }

    fn render_fixed_register_images(&self, device: &Device) -> Result<String> {
        if self.fixed_register_images.is_empty() {
            return Ok(String::new());
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared writes of fixed complete-register images.\n\
             pub mod fixed_register_image {\n",
        );
        for binding in &self.fixed_register_images {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let register_binding = pac_api_svd::register(
                device,
                &binding.name,
                &binding.peripheral,
                &binding.register,
            )?;
            let (index_parameter, index_argument) = if register_binding.is_array {
                (", index: usize", "index")
            } else {
                ("", "")
            };
            output.push_str(&format!(
                "\n    /// Publish the SVD-qualified image `0x{:08x}` to `{}`.`{}`.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}{index_parameter}) {{\n\
                     // SAFETY: generator validation proves that the target is an\n\
                     // ordinary writable 32-bit register, while the SVD extension\n\
                     // and its provenance qualify this exact complete image.\n\
                     unsafe {{\n\
                         registers.{register}({index_argument}).write_with_zero(|writer|\n\
                             writer.bits(0x{:08x})\n\
                         );\n\
                     }}\n\
                 }}\n",
                binding.value, binding.peripheral, binding.register, binding.name, binding.value,
            ));
        }
        output.push_str("}\n");
        Ok(output)
    }

    fn render_register_image_writes(&self, device: &Device) -> Result<String> {
        if self.register_image_writes.is_empty() {
            return Ok(String::new());
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared writes of dynamic complete-register images.\n\
             pub mod register_image_write {\n",
        );
        for binding in &self.register_image_writes {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let register_binding = pac_api_svd::register(
                device,
                &binding.name,
                &binding.peripheral,
                &binding.register,
            )?;
            let (index_parameter, index_argument) = if register_binding.is_array {
                ("index: usize, ", "index")
            } else {
                ("", "")
            };
            output.push_str(&format!(
                "\n    /// Publish a caller-built complete image to `{}`.`{}`.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}, {index_parameter}image: u32) {{\n\
                     // SAFETY: generator validation proves that the target is an\n\
                     // ordinary writable 32-bit register. The SVD extension and\n\
                     // its provenance qualify this semantic whole-image operation.\n\
                     unsafe {{\n\
                         registers.{register}({index_argument}).write_with_zero(|writer|\n\
                             writer.bits(image)\n\
                         );\n\
                     }}\n\
                 }}\n",
                binding.peripheral, binding.register, binding.name,
            ));
        }
        output.push_str("}\n");
        Ok(output)
    }

    fn render_zero_based_field_writes(&self, device: &Device) -> Result<String> {
        if self.zero_based_field_writes.is_empty() {
            return Ok(String::new());
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared field writes based on an all-zero register image.\n\
             pub mod zero_based_field_write {\n",
        );
        for binding in &self.zero_based_field_writes {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let register_binding = pac_api_svd::register(
                device,
                &binding.name,
                &binding.peripheral,
                &binding.register,
            )?;
            let (index_parameter, index_argument) = if register_binding.is_array {
                ("index: usize, ", "index")
            } else {
                ("", "")
            };
            let fields = binding
                .fields
                .iter()
                .map(|name| {
                    let field = pac_api_svd::field(&binding.name, register_binding.info, name)?;
                    let value_type = match field.bit_width() {
                        1 => "bool",
                        2..=8 => "u8",
                        9..=16 => "u16",
                        17..=32 => "u32",
                        _ => unreachable!("SVD validation rejects invalid field widths"),
                    };
                    Ok((name, value_type))
                })
                .collect::<Result<Vec<_>>>()?;
            let field_list = fields
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let (value_parameters, field_writes) = if fields.len() == 1 {
                let (name, value_type) = fields[0];
                let field_name = member_binding_name(name);
                let write = if value_type == "bool" {
                    format!("writer.{field_name}().bit(value)")
                } else {
                    format!("writer.{field_name}().set(value)")
                };
                (format!("value: {value_type}"), write)
            } else {
                let parameters = fields
                    .iter()
                    .map(|(name, value_type)| {
                        format!("{}_value: {value_type}", member_binding_name(name))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let writes = fields
                    .iter()
                    .map(|(name, value_type)| {
                        let field_name = member_binding_name(name);
                        let method = if *value_type == "bool" { "bit" } else { "set" };
                        format!(".{field_name}().{method}({field_name}_value)")
                    })
                    .collect::<String>();
                (parameters, format!("writer{writes}"))
            };
            output.push_str(&format!(
                "\n    /// Write {field_list} in `{}`.`{}` while publishing zero to every other register bit.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}, {index_parameter}{value_parameters}) {{\n\
                     // SAFETY: the SVD extension explicitly qualifies the zero-based\n\
                     // transaction, and generator validation proves every selected field\n\
                     // accepts every value representable by its public argument type.\n\
                     unsafe {{\n\
                         registers.{register}({index_argument}).write_with_zero(|writer|\n\
                             {field_writes}\n\
                         );\n\
                     }}\n\
                 }}\n",
                binding.peripheral, binding.register, binding.name,
            ));
        }
        output.push_str("}\n");
        Ok(output)
    }

    fn render_zero_register_writes(&self, device: &Device) -> Result<String> {
        if self.zero_register_writes.is_empty() {
            return Ok(String::new());
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared complete-register zero writes.\n\
             pub mod zero_register_write {\n",
        );
        for binding in &self.zero_register_writes {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let register_binding = pac_api_svd::register(
                device,
                &binding.name,
                &binding.peripheral,
                &binding.register,
            )?;
            let (index_parameter, index_argument) = if register_binding.is_array {
                (", index: usize", "index")
            } else {
                ("", "")
            };
            output.push_str(&format!(
                "\n    /// Publish zero to every bit of `{}`.`{}`.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}{index_parameter}) {{\n\
                     // SAFETY: the SVD extension and its provenance explicitly\n\
                     // qualify a complete zero write to this ordinary register.\n\
                     unsafe {{\n\
                         registers.{register}({index_argument}).write_with_zero(|writer| writer);\n\
                     }}\n\
                 }}\n",
                binding.peripheral, binding.register, binding.name,
            ));
        }
        output.push_str("}\n");
        Ok(output)
    }

    fn render_masked_register_modifies(&self, device: &Device) -> Result<String> {
        if self.masked_register_modifies.is_empty() {
            return Ok(String::new());
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared masked read-modify-write transactions.\n\
             pub mod masked_register_modify {\n",
        );
        for binding in &self.masked_register_modifies {
            let peripheral_type = type_binding_name(&binding.peripheral);
            let register = member_binding_name(&binding.register);
            let register_binding = pac_api_svd::register(
                device,
                &binding.name,
                &binding.peripheral,
                &binding.register,
            )?;
            let (index_parameter, index_argument) = if register_binding.is_array {
                ("index: usize, ", "index")
            } else {
                ("", "")
            };
            output.push_str(&format!(
                "\n    /// Preserve mask 0x{:08x}, accept input mask 0x{:08x}, and set 0x{:08x} in {}.{}.\n\
                 #[inline]\n\
                 pub fn {}(registers: &crate::{peripheral_type}, {index_parameter}input: u32) {{\n\
                     registers.{register}({index_argument}).modify(|reader, writer| {{\n\
                         let image = (reader.bits() & 0x{:08x})\n\
                             | (input & 0x{:08x})\n\
                             | 0x{:08x};\n\
                         // SAFETY: generator validation proves the three masks are\n\
                         // disjoint and partition every bit of this ordinary register.\n\
                         unsafe {{ writer.bits(image) }}\n\
                     }});\n\
                 }}\n",
                binding.preserve_mask,
                binding.input_mask,
                binding.set_mask,
                binding.peripheral,
                binding.register,
                binding.name,
                binding.preserve_mask,
                binding.input_mask,
                binding.set_mask,
            ));
        }
        output.push_str("}\n");
        Ok(output)
    }
}

fn remove_dimension_placeholder(value: &str) -> String {
    value.replace("[%s]", "").replace("%s", "")
}

fn member_binding_name(value: &str) -> String {
    remove_dimension_placeholder(value).to_ascii_lowercase()
}

fn type_binding_name(value: &str) -> String {
    let value = remove_dimension_placeholder(value);
    let mut output = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character == '_' || character == '-' {
            capitalize = true;
        } else if capitalize {
            output.push(character.to_ascii_uppercase());
            capitalize = false;
        } else {
            output.push(character.to_ascii_lowercase());
        }
    }
    output
}

fn device_access_api() -> &'static str {
    "\n/// Architecture-specific device-memory ordering primitives.\n\
     pub mod device_access {\n\
         /// Order all preceding and following device-memory accesses.\n\
         #[inline]\n\
         pub fn fence() {\n\
             #[cfg(target_arch = \"riscv32\")]\n\
             // SAFETY: this instruction only orders memory and device accesses.\n\
             unsafe { core::arch::asm!(\"fence iorw, iorw\") }\n\
             #[cfg(target_arch = \"arm\")]\n\
             // SAFETY: this instruction only orders memory and device accesses.\n\
             unsafe { core::arch::asm!(\"dmb sy\") }\n\
             #[cfg(target_arch = \"xtensa\")]\n\
             // SAFETY: this instruction only orders memory and device accesses.\n\
             unsafe { core::arch::asm!(\"memw\") }\n\
             #[cfg(not(any(\n\
                 target_arch = \"riscv32\",\n\
                 target_arch = \"arm\",\n\
                 target_arch = \"xtensa\",\n\
             )))]\n\
             core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);\n\
         }\n\
     }\n"
}
