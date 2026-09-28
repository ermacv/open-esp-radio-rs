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
/// Values at least this long have their end recorded by the scan that finds
/// it, so walking into nested values scans each large value once.
const RECORDED_VALUE: u64 = WORK_BLOCK as u64;
/// Admitted bytes per recorded value end, with the table's node overhead.
const RECORDED_END_COST: u64 = 48;
/// Recorded value ends admitted by the table's first growth.
const RECORDED_ENDS_FIRST: usize = 64;
pub(crate) struct Json<'a> {
    source: &'a dyn ByteSource,
    /// The source's length, fixed for its lifetime.
    len: u64,
    cache: [u8; WORK_BLOCK],
    base: u64,
    count: usize,
    /// End of every recorded large value by its start.
    ends: std::collections::BTreeMap<u64, u64>,
    memory: &'a WorkingMemory,
    /// Recorded ends the reservation admits.
    ends_admitted: usize,
    _ends_reservation: Option<MemoryReservation<'a>>,
}
impl<'a> Json<'a> {
    pub fn new(source: &'a dyn ByteSource, memory: &'a WorkingMemory) -> Self {
        Self {
            source,
            len: source.len(),
            cache: [0; WORK_BLOCK],
            base: 0,
            count: 0,
            ends: std::collections::BTreeMap::new(),
            memory,
            ends_admitted: 0,
            _ends_reservation: None,
        }
    }
    /// Record the end of the value at `start` when it is large.
    #[inline]
    fn record(&mut self, start: u64, end: u64, control: &mut dyn RunControl) -> Result<()> {
        if end - start < RECORDED_VALUE {
            return Ok(());
        }
        if self.ends.len() == self.ends_admitted {
            let count = (self.ends_admitted * 2).max(RECORDED_ENDS_FIRST);
            self._ends_reservation = Some(
                self.memory
                    .reserve(count as u64 * RECORDED_END_COST, control.position())?,
            );
            self.ends_admitted = count;
        }
        self.ends.insert(start, end);
        Ok(())
    }
    /// The first recorded value that starts at or after `offset`.
    fn recorded_from(&self, offset: u64) -> Option<(u64, u64)> {
        self.ends.range(offset..).next().map(|(s, e)| (*s, *e))
    }
    /// Bring `offset` into the cached window; returns its index there.
    fn load(&mut self, offset: u64, control: &mut dyn RunControl) -> Result<usize> {
        if offset >= self.len {
            return Err(integrity("unexpected end of JSON"));
        }
        if offset < self.base || offset >= self.base + self.count as u64 {
            self.base = offset;
            self.count = (self.len - offset).min(WORK_BLOCK as u64) as usize;
            self.source
                .read_at(offset, &mut self.cache[..self.count], control)?;
        }
        Ok((offset - self.base) as usize)
    }
    fn byte(&mut self, offset: u64, control: &mut dyn RunControl) -> Result<u8> {
        let index = self.load(offset, control)?;
        Ok(self.cache[index])
    }
    /// The offset of the first byte at or after `offset` that `stop` selects,
    /// scanning whole cached windows; the end of the source is an error.
    fn find(
        &mut self,
        mut offset: u64,
        stop: impl Fn(u8) -> bool,
        control: &mut dyn RunControl,
    ) -> Result<(u64, u8)> {
        loop {
            let index = self.load(offset, control)?;
            let window = &self.cache[index..self.count];
            match window.iter().position(|b| stop(*b)) {
                Some(i) => return Ok((offset + i as u64, window[i])),
                None => offset += window.len() as u64,
            }
        }
    }
    fn whitespace(&mut self, mut offset: u64, control: &mut dyn RunControl) -> Result<u64> {
        while offset < self.len
            && matches!(self.byte(offset, control)?, b' ' | b'\n' | b'\r' | b'\t')
        {
            offset += 1;
        }
        Ok(offset)
    }
    fn string_end(&mut self, mut offset: u64, control: &mut dyn RunControl) -> Result<u64> {
        offset += 1;
        loop {
            let (at, byte) = self.find(offset, |b| b == b'"' || b == b'\\' || b < 32, control)?;
            match byte {
                b'"' => return Ok(at + 1),
                b'\\' => {
                    // The escaped byte must exist; it is never a terminator.
                    self.byte(at + 1, control)?;
                    offset = at + 2;
                }
                _ => return Err(integrity("control character in JSON string")),
            }
        }
    }
    fn node(&mut self, start: u64, control: &mut dyn RunControl) -> Result<Node> {
        control.checkpoint(1)?;
        let start = self.whitespace(start, control)?;
        let kind = self.byte(start, control)?;
        if let Some(end) = self.ends.get(&start) {
            return Ok(Node {
                start,
                end: *end,
                kind,
            });
        }
        let mut end = start;
        if kind == b'"' {
            end = self.string_end(start, control)?;
            self.record(start, end, control)?;
        } else if matches!(kind, b'[' | b'{') {
            let mut stack = [0u8; 128];
            let mut starts = [0u64; 128];
            let mut depth = 0;
            // Recorded values start where the scan lands on their first
            // byte; values recorded during this scan lie behind it.
            let mut recorded = self.recorded_from(start + 1);
            loop {
                let (at, byte) = self.find(
                    end,
                    |b| matches!(b, b'"' | b'[' | b'{' | b']' | b'}'),
                    control,
                )?;
                end = at;
                if let Some((next, next_end)) = recorded
                    && next <= at
                {
                    recorded = self.recorded_from(at + 1);
                    // A recorded nested value was scanned already.
                    if next == at {
                        end = next_end;
                        continue;
                    }
                }
                if byte == b'"' {
                    end = self.string_end(end, control)?;
                    self.record(at, end, control)?;
                    continue;
                }
                match byte {
                    b'[' | b'{' => {
                        if depth == stack.len() {
                            return Err(integrity("JSON nesting exceeds schema decoder limit"));
                        }
                        stack[depth] = if byte == b'[' { b']' } else { b'}' };
                        starts[depth] = at;
                        depth += 1;
                    }
                    b']' | b'}' => {
                        if depth == 0 || stack[depth - 1] != byte {
                            return Err(integrity("mismatched JSON delimiter"));
                        }
                        depth -= 1;
                        self.record(starts[depth], at + 1, control)?;
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
            while end < self.len
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
        if self.whitespace(root.end, control)? != self.len {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A document whose escapes, strings and nested containers straddle the
    /// scanner's window boundaries.
    fn straddling() -> Vec<u8> {
        let mut json = b"{\"pad\":\"".to_vec();
        json.resize(WORK_BLOCK - 1, b'x');
        json.extend_from_slice(b"\\\"\",\"list\":[[\"a]\\\\\",{\"k\":\"}\"}],");
        json.resize(2 * WORK_BLOCK + 3, b' ');
        json.extend_from_slice(b"1],\"n\":-2}");
        json
    }

    #[test]
    fn window_scanning_finds_the_same_value_ends() {
        let bytes = straddling();
        let source: &[u8] = &bytes;
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut json = Json::new(&source, &memory);
        let root = json.root(&mut || Ok(())).unwrap();
        assert_eq!((root.start, root.end), (0, bytes.len() as u64));
        let [pad, list, n] = json
            .fields(root, ["pad", "list", "n"], &mut || Ok(()))
            .unwrap();
        assert_eq!(pad.end, WORK_BLOCK as u64 + 2);
        assert_eq!(bytes[list.end as usize - 1], b']');
        assert_eq!(&bytes[n.start as usize..n.end as usize], b"-2");
        let mut array = json.array(list).unwrap();
        let mut kinds = vec![];
        while let Some(item) = json.next(&mut array, &mut || Ok(())).unwrap() {
            kinds.push(item.kind);
        }
        assert_eq!(kinds, [b'[', b'1']);
    }

    #[test]
    fn unterminated_strings_and_control_characters_are_rejected() {
        for document in [&b"\"abc"[..], b"\"a\\", b"\"a\nb\""] {
            let source: &[u8] = document;
            let memory = WorkingMemory::new(1 << 20).unwrap();
            assert_eq!(
                Json::new(&source, &memory)
                    .root(&mut || Ok(()))
                    .err()
                    .unwrap()
                    .code,
                ErrorCode::Integrity
            );
        }
    }

    /// A source that counts the bytes read from it.
    struct Counted<'a> {
        bytes: &'a [u8],
        read: std::cell::Cell<u64>,
    }
    impl ByteSource for Counted<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(
            &self,
            offset: u64,
            bytes: &mut [u8],
            control: &mut dyn RunControl,
        ) -> Result<()> {
            self.read.set(self.read.get() + bytes.len() as u64);
            self.bytes.read_at(offset, bytes, control)
        }
    }

    #[test]
    fn walking_into_nested_values_scans_each_large_value_once() {
        // Six levels of `{"v":[<level>, "<padding>"]}`: every level is larger
        // than a recorded value, so without recorded ends each nested field
        // would rescan everything below it.
        const LEVELS: usize = 6;
        const PADDING: usize = 16 * WORK_BLOCK;
        let mut document = b"0".to_vec();
        for _ in 0..LEVELS {
            let mut level = b"{\"v\":[".to_vec();
            level.extend_from_slice(&document);
            level.extend_from_slice(b",\"");
            level.resize(level.len() + PADDING, b'x');
            level.extend_from_slice(b"\"]}");
            document = level;
        }
        let source = Counted {
            bytes: &document,
            read: Default::default(),
        };
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut json = Json::new(&source, &memory);
        let mut node = json.root(&mut || Ok(())).unwrap();
        for _ in 0..LEVELS {
            let [values] = json.fields(node, ["v"], &mut || Ok(())).unwrap();
            let mut values = json.array(values).unwrap();
            node = json.next(&mut values, &mut || Ok(())).unwrap().unwrap();
            let padding = json.next(&mut values, &mut || Ok(())).unwrap().unwrap();
            assert_eq!(padding.end - padding.start, PADDING as u64 + 2);
            assert!(json.next(&mut values, &mut || Ok(())).unwrap().is_none());
        }
        assert_eq!(&document[node.start as usize..node.end as usize], b"0");
        // One scan, plus the windows the field and separator checks reload;
        // rescanning each level would read the document about eight times.
        assert!(
            source.read.get() < 2 * document.len() as u64,
            "{} bytes read for a {}-byte document",
            source.read.get(),
            document.len()
        );
    }
}
