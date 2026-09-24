//! Bounded indexes over one saved function. No storage or target-selection authority.
use blobray_domain::*;
pub struct CallObservation {
    pub record: u64,
    pub offset: u64,
    pub call: bool,
    pub target: AbstractValue,
    pub saved_resolution: Option<FunctionAnalysisId>,
}
pub struct MemoryObservation<'a> {
    pub record: u64,
    pub origin: Option<&'a FunctionAnalysisId>,
    pub offset: u64,
    pub access: MemoryKind,
    pub width: u8,
    pub address: &'a AbstractValue,
    pub value: Option<&'a AbstractValue>,
}
pub struct Facts<'a, 'm> {
    records: &'a [FunctionRecord],
    expressions: AdmittedVec<'m, &'a Expression>,
    references: AdmittedVec<'m, &'a ReferenceTarget>,
    inputs: AdmittedVec<'m, (u64, &'a [AbstractValue], u64)>,
    transfers: AdmittedVec<'m, (u64, u64, &'a AbstractValue, bool)>,
    resolutions: AdmittedVec<'m, (u64, &'a Option<FunctionAnalysisId>)>,
}
fn integrity(s: &str) -> Error {
    Error::new(ErrorCode::Integrity, s)
}
impl<'a, 'm> Facts<'a, 'm> {
    pub fn new(
        records: &'a [FunctionRecord],
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            records,
            expressions: AdmittedVec::new(memory),
            references: AdmittedVec::new(memory),
            inputs: AdmittedVec::new(memory),
            transfers: AdmittedVec::new(memory),
            resolutions: AdmittedVec::new(memory),
        };
        let mut instructions = AdmittedVec::new(memory);
        for (record, r) in records.iter().enumerate() {
            c.checkpoint(1)?;
            match r {
                FunctionRecord::Instruction { offset, .. } => {
                    instructions.push(*offset, c.position())?
                }
                FunctionRecord::Expression { id, expression, .. } => {
                    if *id as usize != out.expressions.len() {
                        return Err(integrity("noncanonical navigation expression IDs"));
                    }
                    let earlier = |v: &AbstractValue| !matches!(v,AbstractValue::Expression { id: other } if other >= id);
                    if !match expression {
                        Expression::Integer { left, right, .. } => earlier(left) && earlier(right),
                        Expression::Load { address, .. } => earlier(address),
                        _ => true,
                    } {
                        return Err(integrity("forward or cyclic navigation expression"));
                    }
                    out.expressions.push(expression, c.position())?;
                }
                FunctionRecord::Reference { target, .. } => {
                    out.references.push(target, c.position())?
                }
                FunctionRecord::CallInputs { offset, registers } => out
                    .inputs
                    .push((*offset, registers, record as u64), c.position())?,
                FunctionRecord::Transfer {
                    offset,
                    target,
                    call,
                } => out
                    .transfers
                    .push((*offset, record as u64, target, *call), c.position())?,
                FunctionRecord::CallResolution {
                    offset, analysis, ..
                } => out.resolutions.push((*offset, analysis), c.position())?,
                _ => (),
            }
        }
        c.checkpoint(records.len() as u64 * (records.len().max(1).ilog2() as u64 + 1))?;
        out.references
            .sort_unstable_by(|a, b| a.symbol.cmp(&b.symbol));
        instructions.sort_unstable();
        out.inputs.sort_unstable_by_key(|x| x.0);
        out.transfers.sort_unstable_by_key(|x| x.0);
        out.resolutions.sort_unstable_by_key(|x| x.0);
        if instructions.windows(2).any(|w| w[0] == w[1])
            || out.inputs.windows(2).any(|w| w[0].0 == w[1].0)
            || out.transfers.windows(2).any(|w| w[0].0 == w[1].0)
            || out.resolutions.windows(2).any(|w| w[0].0 == w[1].0)
            || out
                .references
                .windows(2)
                .any(|w| w[0].symbol == w[1].symbol && w[0] != w[1])
        {
            return Err(integrity("duplicate or inconsistent navigation facts"));
        }
        Ok(out)
    }
    pub fn reference(
        &self,
        symbol: &SymbolId,
        c: &mut dyn RunControl,
    ) -> Result<Option<&ReferenceTarget>> {
        c.checkpoint(self.references.len().max(1).ilog2() as u64 + 1)?;
        Ok(self
            .references
            .binary_search_by(|r| r.symbol.cmp(symbol))
            .ok()
            .map(|i| self.references[i]))
    }
    pub(crate) fn expression(&self, id: u32) -> Result<&Expression> {
        self.expressions
            .get(id as usize)
            .copied()
            .ok_or_else(|| integrity("expression ID is absent"))
    }
    pub fn call_inputs_record(&self, offset: u64, c: &mut dyn RunControl) -> Result<Option<u64>> {
        c.checkpoint(self.inputs.len().max(1).ilog2() as u64 + 1)?;
        Ok(self
            .inputs
            .binary_search_by_key(&offset, |r| r.0)
            .ok()
            .map(|i| self.inputs[i].2))
    }
    pub fn call_argument(
        &self,
        offset: u64,
        word: u8,
        c: &mut dyn RunControl,
    ) -> Result<Option<&AbstractValue>> {
        c.checkpoint(self.inputs.len().max(1).ilog2() as u64 + 1)?;
        Ok((word < 8)
            .then(|| {
                self.inputs
                    .binary_search_by_key(&offset, |r| r.0)
                    .ok()
                    .and_then(|i| self.inputs[i].1.get(usize::from(word) + 10))
            })
            .flatten())
    }
    pub fn paths(
        &self,
        value: &AbstractValue,
        recipe: &FunctionRecipe,
        abi: Option<CallAbi>,
        c: &mut dyn RunControl,
    ) -> Result<(Vec<AccessPath>, Option<AccessIssue>)> {
        crate::paths::address_paths(value, &self.expressions, recipe, abi, c)
    }
    fn indirect(
        &self,
        v: &AbstractValue,
        displacement: i32,
        recipe: &FunctionRecipe,
        c: &mut dyn RunControl,
    ) -> Result<AbstractValue> {
        let mut v = v.clone();
        if let AbstractValue::Symbol { symbol, addend } = &v
            && let Some(reference) = self.reference(symbol, c)?
            && reference.definition == SymbolDefinition::Section
        {
            let offset = i128::from(reference.offset) + i128::from(*addend);
            if let Ok(address) = u32::try_from(offset) {
                v = if recipe.address_space == CodeAddressSpace::Image {
                    AbstractValue::ImageAddress { address }
                } else if let Some(section) = reference.section {
                    AbstractValue::Section {
                        section,
                        offset: i64::from(address),
                    }
                } else {
                    AbstractValue::Unknown
                };
            }
        }
        Ok(match v {
            AbstractValue::Constant { value } => AbstractValue::Constant {
                value: value.wrapping_add_signed(displacement) & !1,
            },
            AbstractValue::ImageAddress { address } => AbstractValue::ImageAddress {
                address: address.wrapping_add_signed(displacement) & !1,
            },
            AbstractValue::Section { section, offset } => match u32::try_from(offset) {
                Ok(at) => AbstractValue::Section {
                    section,
                    offset: i64::from(at.wrapping_add_signed(displacement) & !1),
                },
                Err(_) => AbstractValue::Unknown,
            },
            v if displacement == 0 => v,
            _ => AbstractValue::Unknown,
        })
    }
    pub fn calls(
        &self,
        recipe: &FunctionRecipe,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(CallObservation, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        for (record, r) in self.records.iter().enumerate() {
            c.checkpoint(1)?;
            let FunctionRecord::Instruction {
                offset, decoded, ..
            } = r
            else {
                continue;
            };
            c.checkpoint(
                (self.inputs.len() + self.transfers.len() + self.resolutions.len())
                    .max(1)
                    .ilog2() as u64
                    * 3
                    + 3,
            )?;
            let saved = self
                .transfers
                .binary_search_by_key(offset, |x| x.0)
                .ok()
                .map(|i| self.transfers[i]);
            let (record, call, target) = if let Some((_, record, target, call)) = saved {
                (record, call, target.clone())
            } else {
                match decoded.flow {
                    InstructionFlow::Jump { displacement, link } => {
                        let at = i128::from(*offset) + i128::from(displacement);
                        if !link
                            && at >= i128::from(recipe.extent.start)
                            && at
                                < (i128::from(recipe.extent.start)
                                    + i128::from(recipe.extent.length))
                        {
                            continue;
                        }
                        let target = if let Ok(address) = u32::try_from(at) {
                            if recipe.address_space == CodeAddressSpace::Image {
                                AbstractValue::ImageAddress { address }
                            } else {
                                AbstractValue::Section {
                                    section: recipe.section,
                                    offset: i64::from(address),
                                }
                            }
                        } else {
                            AbstractValue::Unknown
                        };
                        (record as u64, link, target)
                    }
                    InstructionFlow::Indirect {
                        base,
                        offset: displacement,
                        link,
                    } => {
                        if !link && base == 1 && displacement == 0 {
                            continue;
                        }
                        let value = self
                            .inputs
                            .binary_search_by_key(offset, |x| x.0)
                            .ok()
                            .and_then(|i| self.inputs[i].1.get(base as usize));
                        let target = match value {
                            Some(v) => self.indirect(v, displacement, recipe, c)?,
                            None => AbstractValue::Unknown,
                        };
                        (record as u64, link, target)
                    }
                    _ => continue,
                }
            };
            let saved_resolution = self
                .resolutions
                .binary_search_by_key(offset, |x| x.0)
                .ok()
                .and_then(|i| self.resolutions[i].1.clone());
            emit(
                CallObservation {
                    record,
                    offset: *offset,
                    call,
                    target,
                    saved_resolution,
                },
                c,
            )?;
        }
        Ok(())
    }
    pub fn accesses(
        &self,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(MemoryObservation<'a>, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        for (record, r) in self.records.iter().enumerate() {
            c.checkpoint(1)?;
            let (origin, offset, access, width, address, value) = match r {
                FunctionRecord::MemoryAccess {
                    offset,
                    access,
                    width,
                    address,
                    value,
                    ..
                } => (None, *offset, *access, *width, address, value.as_ref()),
                FunctionRecord::CalleeEffect {
                    analysis,
                    offset,
                    access,
                    width,
                    address,
                    value,
                    ..
                } => (
                    Some(analysis),
                    *offset,
                    *access,
                    *width,
                    address,
                    value.as_ref(),
                ),
                _ => continue,
            };
            emit(
                MemoryObservation {
                    record: record as u64,
                    origin,
                    offset,
                    access,
                    width,
                    address,
                    value,
                },
                c,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_expression_graphs_and_duplicate_inputs_fail_before_delivery() {
        let memory = WorkingMemory::new(8192).unwrap();
        for records in [
            vec![FunctionRecord::Expression {
                id: 0,
                offset: 0,
                origin: None,
                expression: Expression::Load {
                    address: AbstractValue::Expression { id: 0 },
                    width: 4,
                    signed: false,
                },
            }],
            vec![FunctionRecord::Expression {
                id: 1,
                offset: 0,
                origin: None,
                expression: Expression::EntryRegister { register: 10 },
            }],
            vec![
                FunctionRecord::CallInputs {
                    offset: 0,
                    registers: vec![],
                },
                FunctionRecord::CallInputs {
                    offset: 0,
                    registers: vec![],
                },
            ],
        ] {
            assert!(
                matches!(Facts::new(&records,&memory,&mut || Ok(())),Err(e) if e.code==ErrorCode::Integrity)
            );
            assert_eq!(memory.used(), 0);
        }
        let memory = WorkingMemory::new(1).unwrap();
        let records = [FunctionRecord::Expression {
            id: 0,
            offset: 0,
            origin: None,
            expression: Expression::EntryRegister { register: 10 },
        }];
        assert!(
            matches!(Facts::new(&records,&memory,&mut || Ok(())),Err(e) if e.code==ErrorCode::ResourceLimited)
        );
        assert_eq!(memory.used(), 0);
    }
}
