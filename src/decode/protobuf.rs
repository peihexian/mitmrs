use crate::decode::util::printable_ratio;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Field {
    pub number: u64,
    pub wire_type: u8,
    pub value: FieldValue,
}

#[derive(Debug, Clone)]
pub enum FieldValue {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    LengthDelimited(Vec<u8>),
}

pub fn decode_message(data: &[u8]) -> Result<Vec<Field>, String> {
    decode_message_depth(data, 0)
}

fn decode_message_depth(data: &[u8], depth: usize) -> Result<Vec<Field>, String> {
    if depth > 16 {
        return Err("protobuf nesting too deep".into());
    }
    if data.is_empty() {
        return Err("empty protobuf message".into());
    }
    let mut pos = 0usize;
    let mut fields = Vec::new();
    while pos < data.len() {
        let key = read_varint(data, &mut pos)?;
        let number = key >> 3;
        let wire = (key & 0x7) as u8;
        if number == 0 {
            return Err("invalid protobuf field number 0".into());
        }
        let value = match wire {
            0 => FieldValue::Varint(read_varint(data, &mut pos)?),
            1 => {
                if pos + 8 > data.len() {
                    return Err("truncated fixed64".into());
                }
                let v = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                FieldValue::Fixed64(v)
            }
            2 => {
                let len = read_varint(data, &mut pos)? as usize;
                if len > data.len().saturating_sub(pos) {
                    return Err("truncated length-delimited field".into());
                }
                let bytes = data[pos..pos + len].to_vec();
                pos += len;
                FieldValue::LengthDelimited(bytes)
            }
            3 => {
                // Deprecated group: skip until matching end-group with nesting.
                let mut group_depth = 1usize;
                while group_depth > 0 {
                    if pos >= data.len() {
                        return Err("unterminated group".into());
                    }
                    let k = read_varint(data, &mut pos)?;
                    match k & 0x7 {
                        0 => {
                            read_varint(data, &mut pos)?;
                        }
                        1 => pos += 8,
                        2 => {
                            let len = read_varint(data, &mut pos)? as usize;
                            pos = pos.saturating_add(len);
                        }
                        3 => group_depth += 1,
                        4 => group_depth -= 1,
                        5 => pos += 4,
                        _ => return Err("invalid group wire type".into()),
                    }
                    if pos > data.len() {
                        return Err("truncated group".into());
                    }
                }
                continue;
            }
            4 => return Err("unexpected end-group".into()),
            5 => {
                if pos + 4 > data.len() {
                    return Err("truncated fixed32".into());
                }
                let v = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
                pos += 4;
                FieldValue::Fixed32(v)
            }
            _ => return Err(format!("unsupported protobuf wire type {wire}")),
        };
        fields.push(Field {
            number,
            wire_type: wire,
            value,
        });
        if fields.len() > 100_000 {
            return Err("too many protobuf fields".into());
        }
    }
    Ok(fields)
}

pub fn read_varint(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        if *pos >= data.len() {
            return Err("truncated varint".into());
        }
        let b = data[*pos];
        *pos += 1;
        value |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint too long".into());
        }
    }
    Ok(value)
}

pub fn tree_value(fields: &[Field], depth: usize) -> Value {
    json!(fields
        .iter()
        .map(|f| field_value(f, depth))
        .collect::<Vec<_>>())
}

fn field_value(field: &Field, depth: usize) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("field".into(), json!(field.number));
    obj.insert("wire".into(), json!(field.wire_type));
    match &field.value {
        FieldValue::Varint(v) => {
            obj.insert("varint".into(), json!(v));
            obj.insert("int64".into(), json!(*v as i64));
            if *v <= 1 {
                obj.insert("bool".into(), json!(*v == 1));
            }
            if let Some(s) = zigzag(*v) {
                obj.insert("zigzag".into(), json!(s));
            }
        }
        FieldValue::Fixed64(v) => {
            obj.insert("fixed64".into(), json!(v));
            obj.insert("double".into(), json!(f64::from_bits(*v)));
        }
        FieldValue::Fixed32(v) => {
            obj.insert("fixed32".into(), json!(v));
            obj.insert("float".into(), json!(f32::from_bits(*v)));
        }
        FieldValue::LengthDelimited(bytes) => {
            obj.insert("length".into(), json!(bytes.len()));
            let text = String::from_utf8(bytes.clone()).ok();
            let printable = text
                .as_ref()
                .map(|t| {
                    !t.contains('\0')
                        && printable_ratio(t.as_bytes()) > 0.85
                        && t.chars()
                            .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
                })
                .unwrap_or(false);
            if printable {
                if let Some(t) = &text {
                    obj.insert("string".into(), json!(t));
                }
            }
            if depth < 12 && !bytes.is_empty() && !printable {
                if let Ok(nested) = decode_message_depth(bytes, depth + 1) {
                    if plausible_nested(&nested) {
                        obj.insert("message".into(), tree_value(&nested, depth + 1));
                    }
                }
            }
            if !printable {
                if let Some(packed) = try_packed(bytes) {
                    obj.insert("packed".into(), json!(packed));
                }
                obj.insert(
                    "hex".into(),
                    json!(bytes
                        .iter()
                        .take(256)
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()),
                );
            }
        }
    }
    Value::Object(obj)
}

fn plausible_nested(fields: &[Field]) -> bool {
    if fields.is_empty() || fields.len() > 10_000 {
        return false;
    }
    fields
        .iter()
        .all(|f| f.number >= 1 && f.number <= 536_870_911)
        && fields
            .iter()
            .filter(|f| matches!(f.value, FieldValue::LengthDelimited(_)))
            .count()
            <= fields.len()
}

fn try_packed(bytes: &[u8]) -> Option<Vec<u64>> {
    if bytes.len() < 2 {
        return None;
    }
    let mut pos = 0usize;
    let mut out = Vec::new();
    while pos < bytes.len() {
        match read_varint(bytes, &mut pos) {
            Ok(v) => out.push(v),
            Err(_) => return None,
        }
    }
    if out.len() >= 2 {
        Some(out)
    } else {
        None
    }
}

fn zigzag(v: u64) -> Option<i64> {
    if v == 0 {
        return None;
    }
    let decoded = ((v >> 1) as i64) ^ (-((v & 1) as i64));
    if decoded == v as i64 {
        None
    } else {
        Some(decoded)
    }
}

pub fn text_dump(fields: &[Field], indent: usize, out: &mut String) {
    for f in fields {
        let pad = "  ".repeat(indent);
        match &f.value {
            FieldValue::Varint(v) => {
                out.push_str(&format!("{pad}{} (varint): {}\n", f.number, v));
            }
            FieldValue::Fixed64(v) => {
                out.push_str(&format!(
                    "{pad}{} (fixed64): {} / {}\n",
                    f.number,
                    v,
                    f64::from_bits(*v)
                ));
            }
            FieldValue::Fixed32(v) => {
                out.push_str(&format!(
                    "{pad}{} (fixed32): {} / {}\n",
                    f.number,
                    v,
                    f32::from_bits(*v)
                ));
            }
            FieldValue::LengthDelimited(bytes) => {
                if let Ok(s) = std::str::from_utf8(bytes) {
                    if !s.contains('\0') && printable_ratio(bytes) > 0.85 {
                        out.push_str(&format!("{pad}{} (string): {:?}\n", f.number, s));
                        continue;
                    }
                }
                if let Ok(nested) = decode_message_depth(bytes, indent + 1) {
                    if plausible_nested(&nested) {
                        out.push_str(&format!("{pad}{} (message):\n", f.number));
                        text_dump(&nested, indent + 1, out);
                        continue;
                    }
                }
                out.push_str(&format!(
                    "{pad}{} (bytes, {}): {}\n",
                    f.number,
                    bytes.len(),
                    bytes
                        .iter()
                        .take(32)
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ));
            }
        }
    }
}
