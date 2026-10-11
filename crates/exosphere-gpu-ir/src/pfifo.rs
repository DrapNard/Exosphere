// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Maxwell PFIFO / push-buffer packet decoder.
//!
//! Nintendo Switch command buffers use the Tegra/Maxwell GPFIFO command format. This decoder only
//! turns the wire format into deterministic method writes; it deliberately does not interpret 3D,
//! compute, 2D or DMA semantics. Engine-class decoding happens in the next stage.

use std::fmt;

/// Maxwell command packet secondary opcode (bits 29..31 of a command word).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryOpcode {
    Group0UseTertiary = 0,
    Incrementing = 1,
    Group2UseTertiary = 2,
    NonIncrementing = 3,
    Immediate = 4,
    OneIncrement = 5,
    Reserved = 6,
    EndSegment = 7,
}

impl SecondaryOpcode {
    fn decode(value: u32) -> Self {
        match value & 0x7 {
            0 => Self::Group0UseTertiary,
            1 => Self::Incrementing,
            2 => Self::Group2UseTertiary,
            3 => Self::NonIncrementing,
            4 => Self::Immediate,
            5 => Self::OneIncrement,
            6 => Self::Reserved,
            _ => Self::EndSegment,
        }
    }
}

/// One fully expanded Maxwell method write.
///
/// Keeping the subchannel and method separate is important: method 0 (`SetObject`) changes which
/// engine class owns a subchannel, so the state tracker must process writes in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodWrite {
    pub subchannel: u8,
    pub method: u16,
    pub argument: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PfifoError {
    TruncatedPacket {
        word_index: usize,
        expected_arguments: usize,
        available: usize,
    },
    ReservedOpcode {
        word_index: usize,
    },
    UnsupportedTertiaryOpcode {
        word_index: usize,
        secondary: SecondaryOpcode,
        tertiary: u8,
    },
}

impl fmt::Display for PfifoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedPacket {
                word_index,
                expected_arguments,
                available,
            } => write!(
                f,
                "PFIFO packet at word {word_index} needs {expected_arguments} arguments, only {available} remain"
            ),
            Self::ReservedOpcode { word_index } => {
                write!(f, "reserved PFIFO opcode at word {word_index}")
            }
            Self::UnsupportedTertiaryOpcode {
                word_index,
                secondary,
                tertiary,
            } => write!(
                f,
                "unsupported PFIFO tertiary opcode {tertiary} for {secondary:?} at word {word_index}"
            ),
        }
    }
}

impl std::error::Error for PfifoError {}

/// Expands a raw push-buffer into ordered method writes.
///
/// Method numbers are the hardware method indices (not byte offsets). `END_PB_SEGMENT` terminates
/// decoding without consuming subsequent words, matching the hardware packet boundary semantics.
pub fn decode_pushbuffer(words: &[u32]) -> Result<Vec<MethodWrite>, PfifoError> {
    let mut out = Vec::new();
    let mut cursor = 0usize;

    while cursor < words.len() {
        let packet_index = cursor;
        let header = words[cursor];
        cursor += 1;

        let method = (header & 0x0fff) as u16;
        let subchannel = ((header >> 13) & 0x7) as u8;
        let count_or_data = (header >> 16) & 0x1fff;
        let secondary = SecondaryOpcode::decode(header >> 29);

        match secondary {
            SecondaryOpcode::Incrementing => {
                let count = count_or_data as usize;
                require(words, cursor, count, packet_index)?;
                for index in 0..count {
                    out.push(MethodWrite {
                        subchannel,
                        method: method.wrapping_add(index as u16),
                        argument: words[cursor + index],
                    });
                }
                cursor += count;
            }
            SecondaryOpcode::NonIncrementing => {
                let count = count_or_data as usize;
                require(words, cursor, count, packet_index)?;
                for index in 0..count {
                    out.push(MethodWrite {
                        subchannel,
                        method,
                        argument: words[cursor + index],
                    });
                }
                cursor += count;
            }
            SecondaryOpcode::Immediate => {
                out.push(MethodWrite {
                    subchannel,
                    method,
                    argument: count_or_data,
                });
            }
            SecondaryOpcode::OneIncrement => {
                let count = count_or_data as usize;
                require(words, cursor, count, packet_index)?;
                for index in 0..count {
                    out.push(MethodWrite {
                        subchannel,
                        method: if index == 0 {
                            method
                        } else {
                            method.wrapping_add(1)
                        },
                        argument: words[cursor + index],
                    });
                }
                cursor += count;
            }
            SecondaryOpcode::EndSegment => break,
            SecondaryOpcode::Reserved => {
                return Err(PfifoError::ReservedOpcode {
                    word_index: packet_index,
                });
            }
            SecondaryOpcode::Group0UseTertiary | SecondaryOpcode::Group2UseTertiary => {
                let tertiary = ((header >> 16) & 0x3) as u8;
                // Tertiary mode 0 maps to incrementing for group 0 and non-incrementing for group 2.
                // Other modes manipulate subdevice masks and are preserved as unsupported until a
                // title demonstrates that Exosphere needs them.
                if tertiary != 0 {
                    return Err(PfifoError::UnsupportedTertiaryOpcode {
                        word_index: packet_index,
                        secondary,
                        tertiary,
                    });
                }
                let count = ((header >> 18) & 0x7ff) as usize;
                require(words, cursor, count, packet_index)?;
                let increment = secondary == SecondaryOpcode::Group0UseTertiary;
                for index in 0..count {
                    out.push(MethodWrite {
                        subchannel,
                        method: if increment {
                            method.wrapping_add(index as u16)
                        } else {
                            method
                        },
                        argument: words[cursor + index],
                    });
                }
                cursor += count;
            }
        }
    }

    Ok(out)
}

fn require(
    words: &[u32],
    cursor: usize,
    count: usize,
    packet_index: usize,
) -> Result<(), PfifoError> {
    let available = words.len().saturating_sub(cursor);
    if available < count {
        return Err(PfifoError::TruncatedPacket {
            word_index: packet_index,
            expected_arguments: count,
            available,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(method: u16, subchannel: u8, count: u32, opcode: u32) -> u32 {
        u32::from(method) | (u32::from(subchannel) << 13) | (count << 16) | (opcode << 29)
    }

    #[test]
    fn expands_incrementing_packet() {
        let words = [header(0x100, 2, 3, 1), 10, 11, 12];
        let writes = decode_pushbuffer(&words).unwrap();
        assert_eq!(writes.len(), 3);
        assert_eq!(
            writes[0],
            MethodWrite {
                subchannel: 2,
                method: 0x100,
                argument: 10
            }
        );
        assert_eq!(
            writes[2],
            MethodWrite {
                subchannel: 2,
                method: 0x102,
                argument: 12
            }
        );
    }

    #[test]
    fn expands_non_incrementing_packet() {
        let words = [header(0x40, 0, 2, 3), 0xaa, 0xbb];
        let writes = decode_pushbuffer(&words).unwrap();
        assert_eq!(writes[0].method, 0x40);
        assert_eq!(writes[1].method, 0x40);
    }

    #[test]
    fn decodes_immediate_data() {
        let words = [header(0x68b, 0, 0x123, 4)];
        assert_eq!(decode_pushbuffer(&words).unwrap()[0].argument, 0x123);
    }
}
