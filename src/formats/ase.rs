//! Adobe Swatch Exchange (ASE) file format support
//!
//! Provides reading and writing of ASE color palette files.

use crate::support::error::{PsdError, Result};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Write};

/// Color type in ASE files
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AseColorType {
    Global = 0,
    Spot = 1,
    Normal = 2,
}

impl AseColorType {
    fn from_u16(value: u16) -> Result<Self> {
        match value {
            0 => Ok(AseColorType::Global),
            1 => Ok(AseColorType::Spot),
            2 => Ok(AseColorType::Normal),
            _ => Err(PsdError::InvalidAse(format!(
                "Invalid color type: {}",
                value
            ))),
        }
    }
}

/// ASE color definition
#[derive(Debug, Clone)]
pub enum AseColor {
    RGB {
        name: String,
        r: f32,
        g: f32,
        b: f32,
        color_type: AseColorType,
    },
    CMYK {
        name: String,
        c: f32,
        m: f32,
        y: f32,
        k: f32,
        color_type: AseColorType,
    },
    Gray {
        name: String,
        k: f32,
        color_type: AseColorType,
    },
    LAB {
        name: String,
        l: f32,
        a: f32,
        b: f32,
        color_type: AseColorType,
    },
}

impl AseColor {
    pub fn name(&self) -> &str {
        match self {
            AseColor::RGB { name, .. }
            | AseColor::CMYK { name, .. }
            | AseColor::Gray { name, .. }
            | AseColor::LAB { name, .. } => name,
        }
    }
}

/// ASE color group
#[derive(Debug, Clone)]
pub struct AseGroup {
    pub name: String,
    pub colors: Vec<AseColorOrGroup>,
}

/// Entry in an ASE file (either a color or a group)
#[derive(Debug, Clone)]
pub enum AseColorOrGroup {
    Color(AseColor),
    Group(AseGroup),
}

/// ASE file structure
#[derive(Debug, Clone)]
pub struct Ase {
    pub colors: Vec<AseColorOrGroup>,
}

/// Read an ASE file from a reader
pub fn read_ase<R: Read>(reader: R) -> Result<Ase> {
    struct CountReader<R: Read> {
        inner: R,
        count: u64,
    }
    impl<R: Read> Read for CountReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.count += n as u64;
            Ok(n)
        }
    }

    fn read_u16<R: Read>(reader: &mut CountReader<R>) -> Result<u16> {
        reader
            .read_u16::<BigEndian>()
            .map_err(|e| PsdError::InvalidAse(format!("Failed to read u16: {}", e)))
    }
    fn read_u32<R: Read>(reader: &mut CountReader<R>) -> Result<u32> {
        reader
            .read_u32::<BigEndian>()
            .map_err(|e| PsdError::InvalidAse(format!("Failed to read u32: {}", e)))
    }

    let mut reader = CountReader {
        inner: reader,
        count: 0,
    };

    // Read signature
    let mut sig = [0u8; 4];
    reader
        .read_exact(&mut sig)
        .map_err(|e| PsdError::InvalidAse(format!("Failed to read signature: {}", e)))?;

    if &sig != b"ASEF" {
        return Err(PsdError::InvalidAse(format!(
            "Invalid signature: expected ASEF, got {:?}",
            String::from_utf8_lossy(&sig)
        )));
    }

    // Read version
    let version_major = read_u16(&mut reader)?;
    let version_minor = read_u16(&mut reader)?;

    if version_major != 1 || version_minor != 0 {
        return Err(PsdError::InvalidAse(format!(
            "Unsupported version: {}.{}",
            version_major, version_minor
        )));
    }

    // Read blocks count
    let blocks_count = read_u32(&mut reader)?;

    let mut colors = Vec::new();
    let mut group_stack: Vec<AseGroup> = Vec::new();

    for _ in 0..blocks_count {
        let block_start = reader.count;
        let block_type = read_u16(&mut reader)?;
        let length = read_u32(&mut reader)? as u64;
        let payload_start = reader.count;

        match block_type {
            0x0001 => {
                // Color entry
                let name_length = read_u16(&mut reader)?;
                let name = read_unicode_string(&mut reader, name_length as usize)?;

                let mut color_mode = [0u8; 4];
                reader.read_exact(&mut color_mode).map_err(|e| {
                    PsdError::InvalidAse(format!("Failed to read color mode: {}", e))
                })?;

                let color = match &color_mode {
                    b"RGB " => {
                        let r = reader.read_f32::<BigEndian>()?;
                        let g = reader.read_f32::<BigEndian>()?;
                        let b = reader.read_f32::<BigEndian>()?;
                        let color_type = AseColorType::from_u16(read_u16(&mut reader)?)?;

                        AseColor::RGB {
                            name,
                            r,
                            g,
                            b,
                            color_type,
                        }
                    }
                    b"CMYK" => {
                        let c = reader.read_f32::<BigEndian>()?;
                        let m = reader.read_f32::<BigEndian>()?;
                        let y = reader.read_f32::<BigEndian>()?;
                        let k = reader.read_f32::<BigEndian>()?;
                        let color_type = AseColorType::from_u16(read_u16(&mut reader)?)?;

                        AseColor::CMYK {
                            name,
                            c,
                            m,
                            y,
                            k,
                            color_type,
                        }
                    }
                    b"Gray" => {
                        let k = reader.read_f32::<BigEndian>()?;
                        let color_type = AseColorType::from_u16(read_u16(&mut reader)?)?;

                        AseColor::Gray {
                            name,
                            k,
                            color_type,
                        }
                    }
                    b"LAB " => {
                        let l = reader.read_f32::<BigEndian>()?;
                        let a = reader.read_f32::<BigEndian>()?;
                        let b = reader.read_f32::<BigEndian>()?;
                        let color_type = AseColorType::from_u16(read_u16(&mut reader)?)?;

                        AseColor::LAB {
                            name,
                            l,
                            a,
                            b,
                            color_type,
                        }
                    }
                    _ => {
                        return Err(PsdError::InvalidAse(format!(
                            "Invalid color mode: {}",
                            String::from_utf8_lossy(&color_mode)
                        )));
                    }
                };

                let entry = AseColorOrGroup::Color(color);

                if let Some(group) = group_stack.last_mut() {
                    group.colors.push(entry);
                } else {
                    colors.push(entry);
                }
            }
            0xC001 => {
                // Group start
                let name_length = read_u16(&mut reader)?;
                let name = read_unicode_string(&mut reader, name_length as usize)?;

                group_stack.push(AseGroup {
                    name,
                    colors: Vec::new(),
                });
            }
            0xC002 => {
                // Group end
                if let Some(group) = group_stack.pop() {
                    let entry = AseColorOrGroup::Group(group);

                    if let Some(parent_group) = group_stack.last_mut() {
                        parent_group.colors.push(entry);
                    } else {
                        colors.push(entry);
                    }
                } else {
                    return Err(PsdError::InvalidAse(
                        "Group end without group start".to_string(),
                    ));
                }
            }
            _ => {
                return Err(PsdError::InvalidAse(format!(
                    "Unknown block type: 0x{:04X}",
                    block_type
                )));
            }
        }

        // Align to the declared block payload: trailing extension bytes after
        // a known block are skipped rather than misparsed as the next block.
        let consumed = reader.count - payload_start;
        if consumed > length {
            return Err(PsdError::InvalidAse(format!(
                "Block 0x{:04X} overran its declared {} byte payload by {} bytes",
                block_type,
                length,
                consumed - length
            )));
        }
        let extra = (length - consumed) as usize;
        if extra > 0 {
            let mut sink = std::io::sink();
            std::io::copy(&mut reader.by_ref().take(extra as u64), &mut sink).map_err(|e| {
                PsdError::InvalidAse(format!("Failed to skip block padding: {}", e))
            })?;
        }
        debug_assert!(reader.count - block_start == 6 + length);
    }

    if let Some(group) = group_stack.last() {
        return Err(PsdError::InvalidAse(format!(
            "Unclosed group: {}",
            group.name
        )));
    }

    Ok(Ase { colors })
}

/// Read a Unicode string (UTF-16 big-endian, null-terminated).
///
/// The trailing terminator unit is dropped; interior NULs are preserved.
fn read_unicode_string<R: Read>(reader: &mut R, length: usize) -> Result<String> {
    let mut buffer = Vec::with_capacity(length);

    for _ in 0..length {
        let ch = reader
            .read_u16::<BigEndian>()
            .map_err(|e| PsdError::InvalidAse(format!("Failed to read unicode char: {}", e)))?;
        buffer.push(ch);
    }
    if buffer.last() == Some(&0) {
        buffer.pop();
    }

    String::from_utf16(&buffer).map_err(|e| PsdError::InvalidAse(format!("Invalid UTF-16: {}", e)))
}

/// UTF-16 unit count of `s` plus the required null terminator, validated to
/// fit the wire's u16 length field.
fn utf16_name_len(s: &str) -> Result<u16> {
    let units = s.encode_utf16().count();
    let with_terminator = units
        .checked_add(1)
        .ok_or_else(|| PsdError::InvalidAse("Name length overflow".to_string()))?;
    u16::try_from(with_terminator).map_err(|_| {
        PsdError::InvalidAse(format!(
            "Name of {} UTF-16 units exceeds the ASE u16 length limit",
            units
        ))
    })
}

/// Write a Unicode string (UTF-16 big-endian, null-terminated)
fn write_unicode_string<W: Write>(writer: &mut W, s: &str) -> Result<()> {
    let utf16: Vec<u16> = s.encode_utf16().collect();

    for ch in utf16 {
        writer
            .write_u16::<BigEndian>(ch)
            .map_err(|e| PsdError::InvalidAse(format!("Failed to write unicode char: {}", e)))?;
    }

    // Null terminator
    writer
        .write_u16::<BigEndian>(0)
        .map_err(|e| PsdError::InvalidAse(format!("Failed to write null terminator: {}", e)))?;

    Ok(())
}

/// Write an ASE file to a writer
pub fn write_ase<W: Write>(mut writer: W, ase: &Ase) -> Result<()> {
    // Write signature
    writer
        .write_all(b"ASEF")
        .map_err(|e| PsdError::InvalidAse(format!("Failed to write signature: {}", e)))?;

    // Write version (1.0)
    writer.write_u16::<BigEndian>(1)?;
    writer.write_u16::<BigEndian>(0)?;

    // Count blocks
    let block_count = count_blocks(&ase.colors);
    writer.write_u32::<BigEndian>(block_count as u32)?;

    // Write blocks
    write_blocks(&mut writer, &ase.colors)?;

    Ok(())
}

/// Count total number of blocks (colors + group markers)
fn count_blocks(entries: &[AseColorOrGroup]) -> usize {
    let mut count = 0;

    for entry in entries {
        match entry {
            AseColorOrGroup::Color(_) => count += 1,
            AseColorOrGroup::Group(group) => {
                count += 2; // Start and end markers
                count += count_blocks(&group.colors);
            }
        }
    }

    count
}

/// Write blocks recursively
fn write_blocks<W: Write>(writer: &mut W, entries: &[AseColorOrGroup]) -> Result<()> {
    for entry in entries {
        match entry {
            AseColorOrGroup::Color(color) => {
                write_color(writer, color)?;
            }
            AseColorOrGroup::Group(group) => {
                write_group_start(writer, &group.name)?;
                write_blocks(writer, &group.colors)?;
                write_group_end(writer)?;
            }
        }
    }

    Ok(())
}

/// Write a color block
fn write_color<W: Write>(writer: &mut W, color: &AseColor) -> Result<()> {
    writer.write_u16::<BigEndian>(0x0001)?; // Color block type

    let name = color.name();
    // +1 for null terminator; validated to fit the u16 length field.
    let name_utf16_len = utf16_name_len(name)?;
    let name_bytes = name_utf16_len as usize * 2;

    match color {
        AseColor::RGB {
            r,
            g,
            b,
            color_type,
            ..
        } => {
            // Length: name_len(2) + name_chars*2 + mode(4) + values(3*4) + type(2)
            let length = (2 + name_bytes + 4 + 12 + 2) as u32;
            writer.write_u32::<BigEndian>(length)?;

            writer.write_u16::<BigEndian>(name_utf16_len)?;
            write_unicode_string(writer, name)?;

            writer.write_all(b"RGB ")?;
            writer.write_f32::<BigEndian>(*r)?;
            writer.write_f32::<BigEndian>(*g)?;
            writer.write_f32::<BigEndian>(*b)?;
            writer.write_u16::<BigEndian>(*color_type as u16)?;
        }
        AseColor::CMYK {
            c,
            m,
            y,
            k,
            color_type,
            ..
        } => {
            let length = (2 + name_bytes + 4 + 16 + 2) as u32;
            writer.write_u32::<BigEndian>(length)?;

            writer.write_u16::<BigEndian>(name_utf16_len)?;
            write_unicode_string(writer, name)?;

            writer.write_all(b"CMYK")?;
            writer.write_f32::<BigEndian>(*c)?;
            writer.write_f32::<BigEndian>(*m)?;
            writer.write_f32::<BigEndian>(*y)?;
            writer.write_f32::<BigEndian>(*k)?;
            writer.write_u16::<BigEndian>(*color_type as u16)?;
        }
        AseColor::Gray { k, color_type, .. } => {
            let length = (2 + name_bytes + 4 + 4 + 2) as u32;
            writer.write_u32::<BigEndian>(length)?;

            writer.write_u16::<BigEndian>(name_utf16_len)?;
            write_unicode_string(writer, name)?;

            writer.write_all(b"Gray")?;
            writer.write_f32::<BigEndian>(*k)?;
            writer.write_u16::<BigEndian>(*color_type as u16)?;
        }
        AseColor::LAB {
            l,
            a,
            b,
            color_type,
            ..
        } => {
            let length = (2 + name_bytes + 4 + 12 + 2) as u32;
            writer.write_u32::<BigEndian>(length)?;

            writer.write_u16::<BigEndian>(name_utf16_len)?;
            write_unicode_string(writer, name)?;

            writer.write_all(b"LAB ")?;
            writer.write_f32::<BigEndian>(*l)?;
            writer.write_f32::<BigEndian>(*a)?;
            writer.write_f32::<BigEndian>(*b)?;
            writer.write_u16::<BigEndian>(*color_type as u16)?;
        }
    }

    Ok(())
}

/// Write group start marker
fn write_group_start<W: Write>(writer: &mut W, name: &str) -> Result<()> {
    writer.write_u16::<BigEndian>(0xC001)?; // Group start block type

    let name_utf16_len = utf16_name_len(name)?;
    let length = 2 + name_utf16_len as usize * 2;
    writer.write_u32::<BigEndian>(length as u32)?;

    writer.write_u16::<BigEndian>(name_utf16_len)?;
    write_unicode_string(writer, name)?;

    Ok(())
}

/// Write group end marker
fn write_group_end<W: Write>(writer: &mut W) -> Result<()> {
    writer.write_u16::<BigEndian>(0xC002)?; // Group end block type
    writer.write_u32::<BigEndian>(0)?; // No data
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_read_write_ase_roundtrip() {
        let ase = Ase {
            colors: vec![
                AseColorOrGroup::Color(AseColor::RGB {
                    name: "Red".to_string(),
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    color_type: AseColorType::Normal,
                }),
                AseColorOrGroup::Color(AseColor::CMYK {
                    name: "Cyan".to_string(),
                    c: 1.0,
                    m: 0.0,
                    y: 0.0,
                    k: 0.0,
                    color_type: AseColorType::Normal,
                }),
            ],
        };

        let mut buffer = Vec::new();
        write_ase(&mut buffer, &ase).unwrap();

        let cursor = Cursor::new(buffer);
        let read_ase = read_ase(cursor).unwrap();

        assert_eq!(read_ase.colors.len(), 2);
    }

    #[test]
    fn test_ase_color_name() {
        let color = AseColor::RGB {
            name: "Test".to_string(),
            r: 1.0,
            g: 0.5,
            b: 0.0,
            color_type: AseColorType::Normal,
        };

        assert_eq!(color.name(), "Test");
    }

    #[test]
    fn astral_and_interior_nul_names_roundtrip() {
        let ase = Ase {
            colors: vec![
                AseColorOrGroup::Color(AseColor::RGB {
                    name: "🖌 palette".to_string(),
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    color_type: AseColorType::Global,
                }),
                AseColorOrGroup::Color(AseColor::Gray {
                    name: "interior\u{0}nul".to_string(),
                    k: 0.5,
                    color_type: AseColorType::Spot,
                }),
            ],
        };

        let mut buffer = Vec::new();
        write_ase(&mut buffer, &ase).unwrap();
        let read = read_ase(Cursor::new(buffer)).unwrap();
        match &read.colors[0] {
            AseColorOrGroup::Color(AseColor::RGB { name, .. }) => {
                assert_eq!(name, "🖌 palette");
            }
            other => panic!("unexpected entry: {:?}", other),
        }
        match &read.colors[1] {
            AseColorOrGroup::Color(AseColor::Gray { name, .. }) => {
                assert_eq!(name, "interior\u{0}nul");
            }
            other => panic!("unexpected entry: {:?}", other),
        }
    }

    #[test]
    fn block_trailing_extension_bytes_are_skipped() {
        // Build a valid two-color file, then pad the first color's block with
        // four extra bytes by bumping its declared length. The parser must
        // skip the extension instead of misreading it as the next block.
        let ase = Ase {
            colors: vec![
                AseColorOrGroup::Color(AseColor::RGB {
                    name: "Red".to_string(),
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    color_type: AseColorType::Normal,
                }),
                AseColorOrGroup::Color(AseColor::Gray {
                    name: "Gray".to_string(),
                    k: 0.5,
                    color_type: AseColorType::Global,
                }),
            ],
        };
        let mut buffer = Vec::new();
        write_ase(&mut buffer, &ase).unwrap();

        // First block: file header is 12 bytes, then block type (2) and
        // length (4). Length is at bytes 14..18.
        let first_len = u32::from_be_bytes(buffer[14..18].try_into().unwrap());
        buffer[14..18].copy_from_slice(&(first_len + 4).to_be_bytes());
        // Insert four junk bytes right after the first block's declared payload.
        let insert_at = 18 + first_len as usize;
        let mut padded = Vec::new();
        padded.extend_from_slice(&buffer[..insert_at]);
        padded.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        padded.extend_from_slice(&buffer[insert_at..]);

        let read = read_ase(Cursor::new(padded)).unwrap();
        assert_eq!(read.colors.len(), 2);
        match &read.colors[1] {
            AseColorOrGroup::Color(AseColor::Gray { name, .. }) => {
                assert_eq!(name, "Gray");
            }
            other => panic!("second block misparsed: {:?}", other),
        }
    }

    #[test]
    fn overly_long_name_is_rejected_on_write() {
        let ase = Ase {
            colors: vec![AseColorOrGroup::Color(AseColor::RGB {
                name: "x".repeat(70_000),
                r: 1.0,
                g: 0.0,
                b: 0.0,
                color_type: AseColorType::Normal,
            })],
        };
        let mut buffer = Vec::new();
        let err = write_ase(&mut buffer, &ase).unwrap_err();
        assert!(
            err.to_string().contains("u16 length limit"),
            "unexpected error: {}",
            err
        );
    }
}
