//! Bounded JSON navigation over retained bytes. Nodes are file ranges, never a DOM.
//! Container traversal validates separators/keys; typed leaves use serde validation.
use super::*;
use serde::de::DeserializeOwned;
use std::ops::Deref;

#[derive(Clone, Copy)]
pub(crate) struct Node {
    pub start: u64,
    pub end: u64,
    pub kind: u8,
}
pub(crate) struct Decoded<'a, T> {
    value: T,
    _reservation: MemoryReservation<'a>,
}
impl<T> Deref for Decoded<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
pub(crate) struct Json<'a> {
    source: &'a dyn ByteSource,
    cache: [u8; WORK_BLOCK],
    base: u64,
    count: usize,
}
impl<'a> Json<'a> {
    pub fn new(source: &'a dyn ByteSource) -> Self {
        Self {
            source,
            cache: [0; WORK_BLOCK],
            base: 0,
            count: 0,
        }
    }
    fn byte(&mut self, offset: u64, control: &mut dyn RunControl) -> Result<u8> {
        if offset >= self.source.len() {
            return Err(integrity("unexpected end of JSON"));
        }
        if offset < self.base || offset >= self.base + self.count as u64 {
            self.base = offset;
            self.count = (self.source.len() - offset).min(WORK_BLOCK as u64) as usize;
            self.source
                .read_at(offset, &mut self.cache[..self.count], control)?;
        }
        Ok(self.cache[(offset - self.base) as usize])
    }
    fn whitespace(&mut self, mut offset: u64, control: &mut dyn RunControl) -> Result<u64> {
        while offset < self.source.len()
            && matches!(self.byte(offset, control)?, b' ' | b'\n' | b'\r' | b'\t')
        {
            offset += 1;
        }
        Ok(offset)
    }
    fn string_end(&mut self, mut offset: u64, control: &mut dyn RunControl) -> Result<u64> {
        offset += 1;
        loop {
            match self.byte(offset, control)? {
                b'"' => return Ok(offset + 1),
                b'\\' => {
                    offset += 1;
                    self.byte(offset, control)?;
                }
                b if b < 32 => return Err(integrity("control character in JSON string")),
                _ => (),
            }
            offset += 1;
        }
    }
    fn node(&mut self, start: u64, control: &mut dyn RunControl) -> Result<Node> {
        control.checkpoint(1)?;
        let start = self.whitespace(start, control)?;
        let kind = self.byte(start, control)?;
        let mut end = start;
        if kind == b'"' {
            end = self.string_end(start, control)?;
        } else if matches!(kind, b'[' | b'{') {
            let mut stack = [0u8; 128];
            let mut depth = 0;
            loop {
                let byte = self.byte(end, control)?;
                if byte == b'"' {
                    end = self.string_end(end, control)?;
                    continue;
                }
                match byte {
                    b'[' | b'{' => {
                        if depth == stack.len() {
                            return Err(integrity("JSON nesting exceeds schema decoder limit"));
                        }
                        stack[depth] = if byte == b'[' { b']' } else { b'}' };
                        depth += 1;
                    }
                    b']' | b'}' => {
                        if depth == 0 || stack[depth - 1] != byte {
                            return Err(integrity("mismatched JSON delimiter"));
                        }
                        depth -= 1;
                        if depth == 0 {
                            end += 1;
                            break;
                        }
                    }
                    _ => (),
                }
                end += 1;
            }
        } else {
            while end < self.source.len()
                && !matches!(
                    self.byte(end, control)?,
                    b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t'
                )
            {
                end += 1;
            }
            if start == end {
                return Err(integrity("missing JSON value"));
            }
        }
        Ok(Node { start, end, kind })
    }
    /// The value that starts at `start`, which must end exactly at `end`.
    pub fn node_at(&mut self, start: u64, end: u64, control: &mut dyn RunControl) -> Result<Node> {
        let node = self.node(start, control)?;
        if node.start != start || node.end != end {
            return Err(integrity("indexed JSON span is not one value"));
        }
        Ok(node)
    }
    pub fn root(&mut self, control: &mut dyn RunControl) -> Result<Node> {
        let root = self.node(0, control)?;
        if self.whitespace(root.end, control)? != self.source.len() {
            return Err(integrity("trailing JSON data"));
        }
        Ok(root)
    }
    pub fn decode<'m, T: DeserializeOwned>(
        &mut self,
        node: Node,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Decoded<'m, T>> {
        // Only fixed-schema leaves are decoded here. This conservative admission
        // includes serde's temporary string capacity, Vec growth and typed values.
        let size = node.end - node.start;
        let reservation = memory.reserve(
            size.checked_mul(DECODE_EXPANSION)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimited, "JSON record admission overflow")
                })?,
            control.position(),
        )?;
        let bytes = read_scratch(
            &SourceRange::new(self.source, node.start, size)?,
            memory,
            control,
        )?;
        let value = serde_json::from_slice(&bytes).map_err(super::jobs::json)?;
        Ok(Decoded {
            value,
            _reservation: reservation,
        })
    }
    pub fn fields<const N: usize>(
        &mut self,
        node: Node,
        names: [&str; N],
        control: &mut dyn RunControl,
    ) -> Result<[Node; N]> {
        if node.kind != b'{' {
            return Err(integrity("expected JSON object"));
        }
        let mut result = [None; N];
        let mut position = self.whitespace(node.start + 1, control)?;
        if position == node.end - 1 {
            return Err(integrity("missing object fields"));
        }
        loop {
            let key = self.node(position, control)?;
            if key.kind != b'"' || key.end - key.start > 128 {
                return Err(integrity("invalid schema field name"));
            }
            let mut bytes = [0; 128];
            let size = (key.end - key.start) as usize;
            self.source
                .read_at(key.start, &mut bytes[..size], control)?;
            let name: String = serde_json::from_slice(&bytes[..size]).map_err(super::jobs::json)?;
            let index = names
                .iter()
                .position(|n| *n == name)
                .ok_or_else(|| integrity(format!("unknown manifest field {name}")))?;
            if result[index].is_some() {
                return Err(integrity("duplicate manifest field"));
            }
            position = self.whitespace(key.end, control)?;
            if self.byte(position, control)? != b':' {
                return Err(integrity("missing JSON colon"));
            }
            let value = self.node(position + 1, control)?;
            if value.end >= node.end {
                return Err(integrity("field outside parent object"));
            }
            result[index] = Some(value);
            position = self.whitespace(value.end, control)?;
            if position == node.end - 1 {
                break;
            }
            if self.byte(position, control)? != b',' {
                return Err(integrity("missing field separator"));
            }
            position = self.whitespace(position + 1, control)?;
        }
        if result.iter().any(Option::is_none) {
            return Err(integrity("missing manifest field"));
        }
        Ok(result.map(Option::unwrap))
    }
    pub fn array(&self, node: Node) -> Result<Array> {
        if node.kind != b'[' {
            return Err(integrity("expected JSON array"));
        }
        Ok(Array {
            position: node.start + 1,
            end: node.end - 1,
            first: true,
        })
    }
    pub fn next(
        &mut self,
        array: &mut Array,
        control: &mut dyn RunControl,
    ) -> Result<Option<Node>> {
        control.checkpoint(1)?;
        let mut position = self.whitespace(array.position, control)?;
        if position == array.end {
            return Ok(None);
        }
        if !array.first {
            if self.byte(position, control)? != b',' {
                return Err(integrity("missing array separator"));
            }
            position = self.whitespace(position + 1, control)?;
            if position == array.end {
                return Err(integrity("trailing array separator"));
            }
        }
        let value = self.node(position, control)?;
        if value.end > array.end {
            return Err(integrity("array value outside parent"));
        }
        array.position = value.end;
        array.first = false;
        Ok(Some(value))
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Array {
    position: u64,
    end: u64,
    first: bool,
}
