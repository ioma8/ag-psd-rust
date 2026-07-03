//! Descriptor parsing and writing implementation
//!
//! Handles Adobe Photoshop descriptor structures used throughout PSD files.
//! Descriptors are key-value data structures with typed values.

use crate::support::error::{PsdError, Result};
use crate::io::reader::PsdReader;
use crate::io::writer::PsdWriter;
use std::collections::HashMap;
use std::io::{Read, Seek};

/// Descriptor value types
#[derive(Debug, Clone, PartialEq)]
pub enum DescriptorValue {
    Double(f64),
    Float(f32),
    Integer(i32),
    LargeInteger {
        high: u32,
        low: u32,
    },
    Boolean(bool),
    Text(String),
    Enum {
        enum_type: String,
        value: String,
    },
    Class {
        name: String,
        class_id: String,
    },
    Reference(Vec<ReferenceItem>),
    Descriptor(Descriptor),
    List(Vec<DescriptorValue>),
    DataBytes(Vec<u8>),
    UnitFloat {
        units: String,
        value: f64,
    },
    UnitDouble {
        units: String,
        value: f64,
    },
    Property(String),
    GlobalObject(Descriptor),
    Alias(Vec<u8>),
    FilePath {
        sig: String,
        path: String,
    },
    ObjectArray {
        class_id: String,
        items: Vec<ObjectArrayItem>,
    },
}

/// Object array item (ObAr descriptor type)
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectArrayItem {
    pub id: String,
    pub item_type: String,
    pub u_id: String,
    pub values: Vec<f64>,
}

/// Descriptor structure
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub name: String,
    pub class_id: String,
    pub items: HashMap<String, DescriptorValue>,
}

/// Reference item types
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceItem {
    Property {
        name: String,
        class_id: String,
        key_id: String,
    },
    Class {
        name: String,
        class_id: String,
    },
    EnumeratedReference {
        name: String,
        class_id: String,
        type_id: String,
        enum_value: String,
    },
    Offset {
        name: String,
        class_id: String,
        offset: i32,
    },
    Identifier {
        name: String,
        class_id: String,
        id: u32,
    },
    Index {
        name: String,
        class_id: String,
        index: u32,
    },
    Name {
        name: String,
        class_id: String,
        value: String,
    },
}

/// Units map for unit types
pub fn units_map(code: &str) -> Option<&'static str> {
    match code {
        "#Ang" => Some("Angle"),
        "#Rsl" => Some("Density"),
        "#Rlt" => Some("Distance"),
        "#Nne" => Some("None"),
        "#Prc" => Some("Percent"),
        "#Pxl" => Some("Pixels"),
        "#Mlm" => Some("Millimeters"),
        "#Pnt" => Some("Points"),
        "RrPi" => Some("Picas"),
        "RrIn" => Some("Inches"),
        "RrCm" => Some("Centimeters"),
        _ => None,
    }
}

/// Reverse units map
pub fn units_code(units: &str) -> Option<&'static str> {
    match units {
        "Angle" => Some("#Ang"),
        "Density" => Some("#Rsl"),
        "Distance" => Some("#Rlt"),
        "None" => Some("#Nne"),
        "Percent" => Some("#Prc"),
        "Pixels" => Some("#Pxl"),
        "Millimeters" => Some("#Mlm"),
        "Points" => Some("#Pnt"),
        "Picas" => Some("RrPi"),
        "Inches" => Some("RrIn"),
        "Centimeters" => Some("RrCm"),
        _ => None,
    }
}

/// IDs that must always be written as length-prefixed strings even when 4 chars long.
/// Matches the LONG_DESCRIPTOR_IDS set from the TS reference.
fn is_long_descriptor_id(id: &str) -> bool {
    matches!(
        id,
        "warp"
            | "list"
            | "time"
            | "hold"
            | "Comp"
            | "None"
            | "xx"
            | "xy"
            | "yx"
            | "yy"
            | "tx"
            | "ty"
            | "PinP"
            | "PnRt"
            | "PnOv"
            | "PnDp"
            | "xor"
            | "PuX0"
            | "PuX1"
            | "PuX2"
            | "PuX3"
            | "PuY0"
            | "PuY1"
            | "PuY2"
            | "PuY3"
            | "base"
            | "kana"
            | "ruby"
            | "box"
            | "flow"
            | "clio"
            | "trim"
            | "then"
            | "else"
    )
}

impl<R: Read + Seek> PsdReader<R> {
    /// Read ASCII string or class ID (4-char with zero-length prefix or length-prefixed string)
    pub fn read_ascii_string_or_class_id(&mut self) -> Result<String> {
        let length = self.read_u32()? as usize;
        if length == 0 {
            self.read_signature()
        } else {
            self.read_ascii_string(length)
        }
    }

    /// Read class structure (unicode name + class ID)
    pub fn read_class_structure(&mut self) -> Result<(String, String)> {
        let name = self.read_unicode_string()?;
        let class_id = self.read_ascii_string_or_class_id()?;
        Ok((name, class_id))
    }

    /// Read descriptor structure
    pub fn read_descriptor_structure(&mut self) -> Result<Descriptor> {
        let (name, class_id) = self.read_class_structure()?;
        let item_count = self.read_u32()?;

        let mut items = HashMap::new();
        for _ in 0..item_count {
            let key = self.read_ascii_string_or_class_id()?;
            let ostype = self.read_signature()?;
            let value = self.read_ostype(&ostype)?;
            items.insert(key, value);
        }

        Ok(Descriptor {
            name,
            class_id,
            items,
        })
    }

    /// Read OSType value based on type signature
    pub fn read_ostype(&mut self, ostype: &str) -> Result<DescriptorValue> {
        match ostype {
            // obj  and VlLs share the same on-disk format: count + typed items
            "obj " | "VlLs" => {
                let count = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    let item_type = self.read_signature()?;
                    items.push(self.read_ostype(&item_type)?);
                }
                Ok(DescriptorValue::List(items))
            }
            "Objc" => {
                let desc = self.read_descriptor_structure()?;
                Ok(DescriptorValue::Descriptor(desc))
            }
            "GlbO" => {
                let desc = self.read_descriptor_structure()?;
                Ok(DescriptorValue::GlobalObject(desc))
            }
            "doub" => Ok(DescriptorValue::Double(self.read_f64()?)),
            "DBL " => Ok(DescriptorValue::Float(self.read_f32()?)),
            "UntF" => {
                let units = self.read_signature()?;
                let value = self.read_f64()?;
                Ok(DescriptorValue::UnitDouble {
                    units: units_map(&units).unwrap_or(&units).to_string(),
                    value,
                })
            }
            "UnFl" => {
                let units = self.read_signature()?;
                let value = self.read_f32()? as f64;
                Ok(DescriptorValue::UnitFloat {
                    units: units_map(&units).unwrap_or(&units).to_string(),
                    value,
                })
            }
            "long" => Ok(DescriptorValue::Integer(self.read_i32()?)),
            "comp" => {
                // Big-endian 64-bit integer: high word first
                let high = self.read_u32()?;
                let low = self.read_u32()?;
                Ok(DescriptorValue::LargeInteger { high, low })
            }
            "bool" => Ok(DescriptorValue::Boolean(self.read_u8()? != 0)),
            "TEXT" => Ok(DescriptorValue::Text(self.read_unicode_string()?)),
            "enum" => {
                let enum_type = self.read_ascii_string_or_class_id()?;
                let value = self.read_ascii_string_or_class_id()?;
                Ok(DescriptorValue::Enum { enum_type, value })
            }
            "tdta" => {
                let length = self.read_u32()? as usize;
                Ok(DescriptorValue::DataBytes(self.read_bytes(length)?))
            }
            "ObAr" => {
                let _ = self.read_u32()?; // skip
                let (_name, class_id) = self.read_class_structure()?;
                let item_count = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(item_count);
                for _ in 0..item_count {
                    let id = self.read_ascii_string_or_class_id()?;
                    let item_type = self.read_signature()?;
                    let u_id = self.read_signature()?;
                    let len = self.read_u32()? as usize;
                    let mut values = Vec::with_capacity(len);
                    for _ in 0..len {
                        values.push(self.read_f64()?);
                    }
                    items.push(ObjectArrayItem {
                        id,
                        item_type,
                        u_id,
                        values,
                    });
                }
                Ok(DescriptorValue::ObjectArray { class_id, items })
            }
            // Standalone class/reference types
            "type" | "GlbC" | "Clss" => {
                let (name, class_id) = self.read_class_structure()?;
                Ok(DescriptorValue::Class { name, class_id })
            }
            "rele" => {
                let (name, class_id) = self.read_class_structure()?;
                let _offset = self.read_i32()?; // offset value, no separate field
                Ok(DescriptorValue::Class { name, class_id })
            }
            "prop" => {
                let (_name, _class_id) = self.read_class_structure()?;
                let key_id = self.read_ascii_string_or_class_id()?;
                Ok(DescriptorValue::Property(key_id))
            }
            "Enmr" => {
                let (_name, _class_id) = self.read_class_structure()?;
                let type_id = self.read_ascii_string_or_class_id()?;
                let enum_id = self.read_ascii_string_or_class_id()?;
                Ok(DescriptorValue::Enum {
                    enum_type: type_id,
                    value: enum_id,
                })
            }
            "indx" => {
                let (_name, _class_id) = self.read_class_structure()?;
                let index = self.read_u32()?;
                Ok(DescriptorValue::Integer(index as i32))
            }
            "Idnt" => {
                let (_name, _class_id) = self.read_class_structure()?;
                let id = self.read_u32()?;
                Ok(DescriptorValue::Integer(id as i32))
            }
            "name" => {
                let (_name, _class_id) = self.read_class_structure()?;
                let value = self.read_unicode_string()?;
                Ok(DescriptorValue::Text(value))
            }
            "alis" => {
                let length = self.read_u32()? as usize;
                let bytes = self.read_bytes(length)?;
                Ok(DescriptorValue::Alias(bytes))
            }
            "Pth " => {
                // File path: u32 total_byte_length + 4-char sig + u32 pad + unicode chars + null u16
                let total_length = self.read_u32()? as usize;
                let sig = self.read_signature()?;
                let _pad = self.read_u32()?;
                // total_length includes sig(4) + pad(4) + chars*2 + null(2)
                if total_length < 10 {
                    return Ok(DescriptorValue::FilePath {
                        sig,
                        path: String::new(),
                    });
                }
                let byte_len = total_length - 8; // bytes after sig+pad
                let char_count = byte_len / 2 - 1; // subtract null terminator
                let mut path = String::new();
                for _ in 0..char_count {
                    let ch = self.read_u16()?;
                    path.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
                }
                let _null = self.read_u16()?;
                Ok(DescriptorValue::FilePath { sig, path })
            }
            _ => Err(PsdError::UnsupportedFeature(format!(
                "Unknown OSType: {}",
                ostype
            ))),
        }
    }

    /// Read reference structure (kept for compat; `obj ` now uses read_ostype instead)
    pub fn read_reference_structure(&mut self) -> Result<Vec<ReferenceItem>> {
        let item_count = self.read_u32()? as usize;
        let mut items = Vec::with_capacity(item_count);
        for _ in 0..item_count {
            let ostype = self.read_signature()?;
            let item = match ostype.as_str() {
                "prop" => {
                    let (name, class_id) = self.read_class_structure()?;
                    let key_id = self.read_ascii_string_or_class_id()?;
                    ReferenceItem::Property {
                        name,
                        class_id,
                        key_id,
                    }
                }
                "Clss" => {
                    let (name, class_id) = self.read_class_structure()?;
                    ReferenceItem::Class { name, class_id }
                }
                "Enmr" => {
                    let (name, class_id) = self.read_class_structure()?;
                    let type_id = self.read_ascii_string_or_class_id()?;
                    let enum_value = self.read_ascii_string_or_class_id()?;
                    ReferenceItem::EnumeratedReference {
                        name,
                        class_id,
                        type_id,
                        enum_value,
                    }
                }
                "rele" => {
                    let (name, class_id) = self.read_class_structure()?;
                    let offset = self.read_i32()?;
                    ReferenceItem::Offset {
                        name,
                        class_id,
                        offset,
                    }
                }
                "Idnt" => {
                    let (name, class_id) = self.read_class_structure()?;
                    let id = self.read_u32()?;
                    ReferenceItem::Identifier { name, class_id, id }
                }
                "indx" => {
                    let (name, class_id) = self.read_class_structure()?;
                    let index = self.read_u32()?;
                    ReferenceItem::Index {
                        name,
                        class_id,
                        index,
                    }
                }
                "name" => {
                    let (name, class_id) = self.read_class_structure()?;
                    let value = self.read_unicode_string()?;
                    ReferenceItem::Name {
                        name,
                        class_id,
                        value,
                    }
                }
                _ => {
                    return Err(PsdError::UnsupportedFeature(format!(
                        "Unknown reference type: {}",
                        ostype
                    )))
                }
            };
            items.push(item);
        }
        Ok(items)
    }

    /// Read version and descriptor (common pattern in PSD)
    pub fn read_version_and_descriptor(&mut self) -> Result<Descriptor> {
        let _version = self.read_u32()?;
        self.read_descriptor_structure()
    }
}

impl PsdWriter {
    /// Write ASCII string or class ID
    pub fn write_ascii_string_or_class_id(&mut self, value: &str) -> Result<()> {
        if value.len() == 4 && !is_long_descriptor_id(value) {
            self.write_u32(0)?;
            self.write_signature(value)?;
        } else if value.len() < 4 && !is_long_descriptor_id(value) {
            // Short IDs: zero-length prefix + pad to 4 bytes with spaces
            self.write_u32(0)?;
            let mut padded = value.to_string();
            while padded.len() < 4 {
                padded.push(' ');
            }
            self.write_signature(&padded)?;
        } else {
            self.write_u32(value.len() as u32)?;
            self.write_bytes(value.as_bytes())?;
        }
        Ok(())
    }

    /// Write class structure
    pub fn write_class_structure(&mut self, name: &str, class_id: &str) -> Result<()> {
        self.write_unicode_string_with_padding(name)?;
        self.write_ascii_string_or_class_id(class_id)?;
        Ok(())
    }

    /// Write descriptor structure
    pub fn write_descriptor_structure(&mut self, desc: &Descriptor) -> Result<()> {
        self.write_class_structure(&desc.name, &desc.class_id)?;
        self.write_u32(desc.items.len() as u32)?;

        let mut keys: Vec<_> = desc.items.keys().collect();
        keys.sort();

        for key in keys {
            let value = &desc.items[key];
            self.write_ascii_string_or_class_id(key)?;
            let type_sig = ostype_sig(value);
            self.write_signature(type_sig)?;
            self.write_ostype(value)?;
        }

        Ok(())
    }

    /// Write OSType value
    pub fn write_ostype(&mut self, value: &DescriptorValue) -> Result<()> {
        match value {
            DescriptorValue::Double(v) => self.write_f64(*v)?,
            DescriptorValue::Float(v) => self.write_f32(*v)?,
            DescriptorValue::Integer(v) => self.write_i32(*v)?,
            DescriptorValue::LargeInteger { high, low } => {
                // Big-endian: high word first
                self.write_u32(*high)?;
                self.write_u32(*low)?;
            }
            DescriptorValue::Boolean(v) => self.write_u8(if *v { 1 } else { 0 })?,
            DescriptorValue::Text(s) => self.write_unicode_string_with_padding(s)?,
            DescriptorValue::Enum { enum_type, value } => {
                // Write as "Enmr" format: class_structure(name, class_id) + type_id + value
                self.write_class_structure("", enum_type)?;
                self.write_ascii_string_or_class_id(enum_type)?;
                self.write_ascii_string_or_class_id(value)?;
            }
            DescriptorValue::Class { name, class_id } => {
                self.write_class_structure(name, class_id)?;
            }
            DescriptorValue::Reference(items) => {
                self.write_reference_structure(items)?;
            }
            DescriptorValue::Descriptor(desc) => {
                self.write_descriptor_structure(desc)?;
            }
            DescriptorValue::List(items) => {
                self.write_u32(items.len() as u32)?;
                for item in items {
                    let type_sig = ostype_sig(item);
                    self.write_signature(type_sig)?;
                    self.write_ostype(item)?;
                }
            }
            DescriptorValue::DataBytes(data) => {
                self.write_u32(data.len() as u32)?;
                self.write_bytes(data)?;
            }
            DescriptorValue::UnitFloat { units, value } => {
                let code = units_code(units).unwrap_or("#Pxl");
                self.write_signature(code)?;
                self.write_f32(*value as f32)?;
            }
            DescriptorValue::UnitDouble { units, value } => {
                let code = units_code(units).unwrap_or("#Pxl");
                self.write_signature(code)?;
                self.write_f64(*value)?;
            }
            DescriptorValue::Property(key) => {
                // Write as prop: empty name+classID + key
                self.write_class_structure("", "")?;
                self.write_ascii_string_or_class_id(key)?;
            }
            DescriptorValue::Alias(bytes) => {
                self.write_u32(bytes.len() as u32)?;
                self.write_bytes(bytes)?;
            }
            DescriptorValue::GlobalObject(desc) => {
                self.write_descriptor_structure(desc)?;
            }
            DescriptorValue::FilePath { sig, path } => {
                // total_length = 4 (sig) + 4 (pad) + chars*2 + 2 (null)
                let byte_len = path.chars().count() * 2 + 2 + 8;
                self.write_u32(byte_len as u32)?;
                self.write_signature(sig)?;
                self.write_u32(0)?; // pad
                for ch in path.chars() {
                    self.write_u16(ch as u16)?;
                }
                self.write_u16(0)?; // null terminator
            }
            DescriptorValue::ObjectArray { class_id, items } => {
                self.write_u32(16)?;
                self.write_class_structure("", class_id)?;
                self.write_u32(items.len() as u32)?;
                for item in items {
                    self.write_ascii_string_or_class_id(&item.id)?;
                    self.write_signature(&item.item_type)?;
                    self.write_signature(&item.u_id)?;
                    self.write_u32(item.values.len() as u32)?;
                    for &v in &item.values {
                        self.write_f64(v)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Write reference structure
    pub fn write_reference_structure(&mut self, items: &[ReferenceItem]) -> Result<()> {
        self.write_u32(items.len() as u32)?;
        for item in items {
            match item {
                ReferenceItem::Property {
                    name,
                    class_id,
                    key_id,
                } => {
                    self.write_signature("prop")?;
                    self.write_class_structure(name, class_id)?;
                    self.write_ascii_string_or_class_id(key_id)?;
                }
                ReferenceItem::Class { name, class_id } => {
                    self.write_signature("Clss")?;
                    self.write_class_structure(name, class_id)?;
                }
                ReferenceItem::EnumeratedReference {
                    name,
                    class_id,
                    type_id,
                    enum_value,
                } => {
                    self.write_signature("Enmr")?;
                    self.write_class_structure(name, class_id)?;
                    self.write_ascii_string_or_class_id(type_id)?;
                    self.write_ascii_string_or_class_id(enum_value)?;
                }
                ReferenceItem::Offset {
                    name,
                    class_id,
                    offset,
                } => {
                    self.write_signature("rele")?;
                    self.write_class_structure(name, class_id)?;
                    self.write_i32(*offset)?;
                }
                ReferenceItem::Identifier { name, class_id, id } => {
                    self.write_signature("Idnt")?;
                    self.write_class_structure(name, class_id)?;
                    self.write_u32(*id)?;
                }
                ReferenceItem::Index {
                    name,
                    class_id,
                    index,
                } => {
                    self.write_signature("indx")?;
                    self.write_class_structure(name, class_id)?;
                    self.write_u32(*index)?;
                }
                ReferenceItem::Name {
                    name,
                    class_id,
                    value,
                } => {
                    self.write_signature("name")?;
                    self.write_class_structure(name, class_id)?;
                    self.write_unicode_string_with_padding(value)?;
                }
            }
        }
        Ok(())
    }

    /// Write version and descriptor
    pub fn write_version_and_descriptor(&mut self, version: u32, desc: &Descriptor) -> Result<()> {
        self.write_u32(version)?;
        self.write_descriptor_structure(desc)?;
        Ok(())
    }
}

/// Return the ostype signature string for a DescriptorValue
fn ostype_sig(value: &DescriptorValue) -> &'static str {
    match value {
        DescriptorValue::Double(_) => "doub",
        DescriptorValue::Float(_) => "DBL ",
        DescriptorValue::Integer(_) => "long",
        DescriptorValue::LargeInteger { .. } => "comp",
        DescriptorValue::Boolean(_) => "bool",
        DescriptorValue::Text(_) => "TEXT",
        DescriptorValue::Enum { .. } => "Enmr",
        DescriptorValue::Class { .. } => "type",
        DescriptorValue::Reference(_) => "obj ",
        DescriptorValue::Descriptor(_) => "Objc",
        DescriptorValue::GlobalObject(_) => "GlbO",
        DescriptorValue::List(_) => "VlLs",
        DescriptorValue::DataBytes(_) => "tdta",
        DescriptorValue::UnitFloat { .. } => "UnFl",
        DescriptorValue::UnitDouble { .. } => "UntF",
        DescriptorValue::Property(_) => "prop",
        DescriptorValue::Alias(_) => "alis",
        DescriptorValue::FilePath { .. } => "Pth ",
        DescriptorValue::ObjectArray { .. } => "ObAr",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_descriptor_roundtrip() {
        let mut desc = Descriptor {
            name: "Test".to_string(),
            class_id: "TEST".to_string(),
            items: HashMap::new(),
        };

        desc.items
            .insert("lng1".to_string(), DescriptorValue::Integer(42));
        desc.items
            .insert("dbl1".to_string(), DescriptorValue::Double(3.14));
        desc.items
            .insert("bool".to_string(), DescriptorValue::Boolean(true));
        desc.items.insert(
            "txts".to_string(),
            DescriptorValue::Text("Hello".to_string()),
        );

        let mut writer = PsdWriter::new(1024);
        writer.write_descriptor_structure(&desc).unwrap();
        let buffer = writer.into_buffer();

        let cursor = Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());
        let read_desc = reader.read_descriptor_structure().unwrap();

        assert_eq!(read_desc.name, desc.name);
        assert_eq!(read_desc.class_id, desc.class_id);
        assert_eq!(read_desc.items.len(), desc.items.len());
    }

    #[test]
    fn test_unit_float() {
        let mut writer = PsdWriter::new(128);
        let value = DescriptorValue::UnitDouble {
            units: "Pixels".to_string(),
            value: 100.0,
        };

        writer.write_signature("UntF").unwrap();
        writer.write_ostype(&value).unwrap();

        let buffer = writer.into_buffer();
        let cursor = Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let _sig = reader.read_signature().unwrap();
        if let DescriptorValue::UnitDouble { units, value } = reader.read_ostype("UntF").unwrap() {
            assert_eq!(units, "Pixels");
            assert_eq!(value, 100.0);
        } else {
            panic!("Expected UnitDouble");
        }
    }

    #[test]
    fn comp_large_integer_roundtrip() {
        let value = DescriptorValue::LargeInteger {
            high: 0x0000_0001,
            low: 0x8000_0000,
        };
        let mut writer = PsdWriter::new(64);
        writer.write_signature("comp").unwrap();
        writer.write_ostype(&value).unwrap();
        let buf = writer.into_buffer();
        // Verify high word comes first on the wire
        assert_eq!(&buf[4..8], &[0x00, 0x00, 0x00, 0x01]); // high
        assert_eq!(&buf[8..12], &[0x80, 0x00, 0x00, 0x00]); // low
        let mut reader = PsdReader::new(Cursor::new(buf), Default::default());
        let _ = reader.read_signature().unwrap();
        match reader.read_ostype("comp").unwrap() {
            DescriptorValue::LargeInteger { high, low } => {
                assert_eq!(high, 0x0000_0001);
                assert_eq!(low, 0x8000_0000);
            }
            _ => panic!("expected LargeInteger"),
        }
    }

    #[test]
    fn long_descriptor_id_written_as_string() {
        // "Comp" and "None" must be written as length-prefixed strings, not 4-char class IDs
        let mut writer = PsdWriter::new(64);
        writer.write_ascii_string_or_class_id("Comp").unwrap();
        let buf = writer.into_buffer();
        // First 4 bytes are the length (4), not 0x00000000
        assert_eq!(u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]), 4);
    }

    #[test]
    fn obj_list_roundtrip() {
        let list = DescriptorValue::List(vec![
            DescriptorValue::Integer(1),
            DescriptorValue::Integer(2),
        ]);

        let mut writer = PsdWriter::new(256);
        writer.write_signature("obj ").unwrap();
        writer
            .write_ostype(&DescriptorValue::List(vec![
                DescriptorValue::Integer(1),
                DescriptorValue::Integer(2),
            ]))
            .unwrap();

        let buf = writer.into_buffer();
        let mut reader = PsdReader::new(Cursor::new(buf), Default::default());
        let _ = reader.read_signature().unwrap();
        // obj  and VlLs share the same wire format
        match reader.read_ostype("obj ").unwrap() {
            DescriptorValue::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], DescriptorValue::Integer(1));
            }
            _ => panic!("expected List"),
        }
        let _ = list; // suppress unused warning
    }

    #[test]
    fn file_path_roundtrip() {
        let value = DescriptorValue::FilePath {
            sig: "alas".to_string(),
            path: "/tmp/test.psd".to_string(),
        };
        let mut writer = PsdWriter::new(256);
        writer.write_signature("Pth ").unwrap();
        writer.write_ostype(&value).unwrap();
        let buf = writer.into_buffer();
        let mut reader = PsdReader::new(Cursor::new(buf), Default::default());
        let _ = reader.read_signature().unwrap();
        match reader.read_ostype("Pth ").unwrap() {
            DescriptorValue::FilePath { sig, path } => {
                assert_eq!(sig, "alas");
                assert_eq!(path, "/tmp/test.psd");
            }
            _ => panic!("expected FilePath"),
        }
    }

    #[test]
    fn tdta_roundtrip() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00];
        let value = DescriptorValue::DataBytes(data.clone());
        let mut writer = PsdWriter::new(64);
        writer.write_signature("tdta").unwrap();
        writer.write_ostype(&value).unwrap();
        let buf = writer.into_buffer();
        // Length must be written as unsigned u32 = 5
        assert_eq!(u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]), 5);
        let mut reader = PsdReader::new(Cursor::new(buf), Default::default());
        let _ = reader.read_signature().unwrap();
        match reader.read_ostype("tdta").unwrap() {
            DescriptorValue::DataBytes(d) => assert_eq!(d, data),
            _ => panic!("expected RawData"),
        }
    }

    #[test]
    fn object_array_uses_photoshop_version_prefix() {
        let value = DescriptorValue::ObjectArray {
            class_id: "rationalPoint".to_string(),
            items: vec![ObjectArrayItem {
                id: "Hrzn".to_string(),
                item_type: "UnFl".to_string(),
                u_id: "#Pxl".to_string(),
                values: vec![1.0, 2.0],
            }],
        };

        let mut writer = PsdWriter::new(256);
        writer.write_signature("ObAr").unwrap();
        writer.write_ostype(&value).unwrap();
        let buf = writer.into_buffer();

        assert_eq!(u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]), 16);

        let mut reader = PsdReader::new(Cursor::new(buf), Default::default());
        let _ = reader.read_signature().unwrap();
        match reader.read_ostype("ObAr").unwrap() {
            DescriptorValue::ObjectArray { class_id, items } => {
                assert_eq!(class_id, "rationalPoint");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].id, "Hrzn");
                assert_eq!(items[0].values, vec![1.0, 2.0]);
            }
            _ => panic!("expected ObjectArray"),
        }
    }
}
