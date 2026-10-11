// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! CMIF-facing filesystem objects returned by `fsp-srv`.
//!
//! The canonical VFS deliberately knows nothing about HIPC/CMIF. These objects adapt Horizon
//! command IDs to `exosphere-vfs` operations. Buffer-descriptor marshaling belongs to the HIPC
//! layer. Production requests use materialized HIPC buffers; `IpcValue` fallbacks exist only for
//! focused unit tests and compatibility while the IPC surface is expanded.

use crate::{
    error::{HorizonError, HorizonResult},
    ipc::{
        IpcBufferDirection, IpcBufferWrite, IpcValue, ReturnedObject, ServiceRequest,
        ServiceResponse,
    },
};
use exosphere_vfs::{EntryType, GuestPath, OpenMode};

pub mod filesystem_command {
    pub const CREATE_FILE: u32 = 0;
    pub const DELETE_FILE: u32 = 1;
    pub const CREATE_DIRECTORY: u32 = 2;
    pub const DELETE_DIRECTORY: u32 = 3;
    pub const DELETE_DIRECTORY_RECURSIVELY: u32 = 4;
    pub const RENAME_FILE: u32 = 5;
    pub const RENAME_DIRECTORY: u32 = 6;
    pub const GET_ENTRY_TYPE: u32 = 7;
    pub const OPEN_FILE: u32 = 8;
    pub const OPEN_DIRECTORY: u32 = 9;
    pub const COMMIT: u32 = 10;
    pub const GET_FREE_SPACE_SIZE: u32 = 11;
    pub const GET_TOTAL_SPACE_SIZE: u32 = 12;
}

pub mod file_command {
    pub const READ: u32 = 0;
    pub const WRITE: u32 = 1;
    pub const FLUSH: u32 = 2;
    pub const SET_SIZE: u32 = 3;
    pub const GET_SIZE: u32 = 4;
}

pub mod directory_command {
    pub const READ: u32 = 0;
    pub const GET_ENTRY_COUNT: u32 = 1;
}

pub mod storage_command {
    pub const READ: u32 = 0;
    pub const WRITE: u32 = 1;
    pub const FLUSH: u32 = 2;
    pub const SET_SIZE: u32 = 3;
    pub const GET_SIZE: u32 = 4;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsSessionObject {
    FileSystem {
        mount: String,
    },
    File {
        mount: String,
        path: GuestPath,
        mode: OpenMode,
    },
    Directory {
        mount: String,
        path: GuestPath,
        filter: u64,
        cursor: usize,
    },
    Storage {
        name: String,
        writable: bool,
    },
}

pub fn dispatch(
    fs: &mut crate::fs::HorizonFs,
    object: &mut FsSessionObject,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    match object {
        FsSessionObject::FileSystem { mount } => dispatch_filesystem(fs, mount, request),
        FsSessionObject::File { mount, path, mode } => {
            dispatch_file(fs, mount, path, *mode, request)
        }
        FsSessionObject::Directory {
            mount,
            path,
            filter,
            cursor,
        } => dispatch_directory(fs, mount, path, *filter, cursor, request),
        FsSessionObject::Storage { name, writable } => {
            dispatch_storage(fs, name, *writable, request)
        }
    }
}

fn dispatch_filesystem(
    fs: &mut crate::fs::HorizonFs,
    mount: &str,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let mounted = fs
        .mounts
        .get(&exosphere_vfs::MountId(mount.to_owned()))
        .map_err(map_fs)?;
    match request.command_id {
        filesystem_command::CREATE_FILE => {
            ensure_mount_writable(mounted.writable)?;
            let path = input_path(request, 0)?;
            // libnx marshals { u32 option; padding; u64 size }.
            let size = raw_u64(request, 8)?;
            mounted.fs.create_file(&path, size).map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        filesystem_command::DELETE_FILE => {
            ensure_mount_writable(mounted.writable)?;
            mounted.fs.delete_file(&input_path(request, 0)?).map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        filesystem_command::CREATE_DIRECTORY => {
            ensure_mount_writable(mounted.writable)?;
            mounted.fs.create_directory(&input_path(request, 0)?).map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        filesystem_command::DELETE_DIRECTORY => {
            ensure_mount_writable(mounted.writable)?;
            mounted.fs.delete_directory(&input_path(request, 0)?).map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        filesystem_command::RENAME_FILE | filesystem_command::RENAME_DIRECTORY => {
            ensure_mount_writable(mounted.writable)?;
            let from = input_path(request, 0)?;
            let to = input_path(request, 1)?;
            mounted.fs.rename(&from, &to).map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        filesystem_command::GET_ENTRY_TYPE => {
            let stat = mounted.fs.stat(&input_path(request, 0)?).map_err(map_fs)?;
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U32(match stat.entry_type {
                EntryType::Directory => 0,
                EntryType::File => 1,
            }));
            Ok(response)
        }
        filesystem_command::OPEN_FILE => {
            let path = input_path(request, 0)?;
            let mode = OpenMode(raw_u32_or_value(request, 0)? as u8);
            let stat = mounted.fs.stat(&path).map_err(map_fs)?;
            if stat.entry_type != EntryType::File {
                return Err(HorizonError::FileSystem("OpenFile target is not a file".into()));
            }
            if mode.contains(OpenMode::WRITE) && !mounted.writable {
                return Err(HorizonError::PermissionDenied);
            }
            let mut response = ServiceResponse::success();
            response.objects.push(ReturnedObject::File {
                mount: mount.to_owned(),
                path: path.as_str().to_owned(),
                mode: mode.0,
            });
            Ok(response)
        }
        filesystem_command::OPEN_DIRECTORY => {
            let path = input_path(request, 0)?;
            let filter = u64::from(raw_u32_or_value(request, 0)?);
            let stat = mounted.fs.stat(&path).map_err(map_fs)?;
            if stat.entry_type != EntryType::Directory {
                return Err(HorizonError::FileSystem("OpenDirectory target is not a directory".into()));
            }
            let mut response = ServiceResponse::success();
            response.objects.push(ReturnedObject::Directory {
                mount: mount.to_owned(),
                path: path.as_str().to_owned(),
                filter,
            });
            Ok(response)
        }
        filesystem_command::COMMIT => {
            mounted.fs.flush().map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        filesystem_command::GET_FREE_SPACE_SIZE => {
            let path = input_path(request, 0)?;
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(mounted.fs.free_space(&path).map_err(map_fs)?));
            Ok(response)
        }
        filesystem_command::GET_TOTAL_SPACE_SIZE => {
            let path = input_path(request, 0)?;
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(mounted.fs.total_space(&path).map_err(map_fs)?));
            Ok(response)
        }
        filesystem_command::DELETE_DIRECTORY_RECURSIVELY => Err(HorizonError::InvalidState(
            "recursive directory deletion is intentionally not exposed until VFS backends implement it atomically".into(),
        )),
        id => Err(HorizonError::UnknownCommand { service: "IFileSystem".into(), command_id: id }),
    }
}

fn dispatch_file(
    fs: &mut crate::fs::HorizonFs,
    mount: &str,
    path: &GuestPath,
    mode: OpenMode,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let mounted = fs
        .mounts
        .get(&exosphere_vfs::MountId(mount.to_owned()))
        .map_err(map_fs)?;
    match request.command_id {
        file_command::READ => {
            if !mode.contains(OpenMode::READ) {
                return Err(HorizonError::PermissionDenied);
            }
            // FsFileRead input: u32 option, u32 pad, s64 offset, u64 read_size.
            let offset = raw_u64_or_value(request, 8, 0)?;
            let requested = raw_u64_or_value(request, 16, 1)?;
            let (buffer_index, capacity) = output_buffer(request, 0)?;
            let size = requested.min(capacity);
            let host_size = usize::try_from(size).map_err(|_| {
                HorizonError::IpcProtocol("read size does not fit host usize".into())
            })?;
            let mut bytes = vec![0u8; host_size];
            let read = mounted
                .fs
                .read_file(path, offset, &mut bytes)
                .map_err(map_fs)?;
            bytes.truncate(read);
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(read as u64));
            response.buffer_writes.push(IpcBufferWrite {
                buffer_index,
                data: bytes,
            });
            Ok(response)
        }
        file_command::WRITE => {
            if !mode.contains(OpenMode::WRITE) || !mounted.writable {
                return Err(HorizonError::PermissionDenied);
            }
            // FsFileWrite has the same raw input layout as Read and one map-alias input buffer.
            let option = raw_u32_or_value(request, 0)?;
            let offset = raw_u64_or_value(request, 8, 0)?;
            let requested = raw_u64_or_value(request, 16, 1)?;
            let data = input_buffer_bytes(request, 0)?;
            let write_len = usize::try_from(requested.min(data.len() as u64)).map_err(|_| {
                HorizonError::IpcProtocol("write size does not fit host usize".into())
            })?;
            let written = mounted
                .fs
                .write_file(path, offset, &data[..write_len])
                .map_err(map_fs)?;
            if option & 1 != 0 {
                mounted.fs.flush().map_err(map_fs)?;
            }
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(written as u64));
            Ok(response)
        }
        file_command::FLUSH => {
            mounted.fs.flush().map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        file_command::SET_SIZE => {
            if !mode.contains(OpenMode::WRITE) || !mounted.writable {
                return Err(HorizonError::PermissionDenied);
            }
            mounted
                .fs
                .set_file_size(path, raw_u64_or_value(request, 0, 0)?)
                .map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        file_command::GET_SIZE => {
            let stat = mounted.fs.stat(path).map_err(map_fs)?;
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(stat.size));
            Ok(response)
        }
        id => Err(HorizonError::UnknownCommand {
            service: "IFile".into(),
            command_id: id,
        }),
    }
}

fn dispatch_directory(
    fs: &mut crate::fs::HorizonFs,
    mount: &str,
    path: &GuestPath,
    filter: u64,
    cursor: &mut usize,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let mounted = fs
        .mounts
        .get(&exosphere_vfs::MountId(mount.to_owned()))
        .map_err(map_fs)?;
    let entries = mounted.fs.read_directory(path).map_err(map_fs)?;
    let filtered: Vec<_> = entries
        .into_iter()
        .filter(|entry| match entry.entry_type {
            EntryType::Directory => filter & 0x1 != 0,
            EntryType::File => filter & 0x2 != 0,
        })
        .collect();

    match request.command_id {
        directory_command::GET_ENTRY_COUNT => {
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(filtered.len() as u64));
            Ok(response)
        }
        directory_command::READ => {
            let (buffer_index, capacity) = output_buffer(request, 0)?;
            let max_entries = usize::try_from(capacity / 0x310).map_err(|_| {
                HorizonError::IpcProtocol("directory buffer capacity overflow".into())
            })?;
            let end = (*cursor).saturating_add(max_entries).min(filtered.len());
            let slice = &filtered[*cursor..end];
            *cursor = end;
            let mut bytes = Vec::with_capacity(slice.len() * 0x310);
            for entry in slice {
                encode_directory_entry(entry, &mut bytes);
            }
            let mut response = ServiceResponse::success();
            response.values.push(IpcValue::U64(slice.len() as u64));
            response.buffer_writes.push(IpcBufferWrite {
                buffer_index,
                data: bytes,
            });
            Ok(response)
        }
        id => Err(HorizonError::UnknownCommand {
            service: "IDirectory".into(),
            command_id: id,
        }),
    }
}

fn encode_directory_entry(entry: &exosphere_vfs::DirectoryEntry, out: &mut Vec<u8>) {
    let mut encoded = [0u8; 0x310];
    let name = entry.name.as_bytes();
    let count = name.len().min(0x300);
    encoded[..count].copy_from_slice(&name[..count]);
    encoded[0x301] = if entry.entry_type == EntryType::Directory {
        1
    } else {
        0
    };
    encoded[0x304] = if entry.entry_type == EntryType::Directory {
        0
    } else {
        1
    };
    encoded[0x308..0x310].copy_from_slice(&entry.size.to_le_bytes());
    out.extend_from_slice(&encoded);
}

fn dispatch_storage(
    fs: &mut crate::fs::HorizonFs,
    name: &str,
    writable: bool,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let storage = fs.storage(name)?;
    match request.command_id {
        storage_command::READ => {
            // IStorage::Read uses an output HIPC buffer plus offset/length raw arguments.
            let offset = raw_u64_or_value(request, 0, 0)?;
            let requested = raw_u64_or_value(request, 8, 1)?;
            let (buffer_index, capacity) = output_buffer(request, 0)?;
            let size = requested.min(capacity);
            let host_size = usize::try_from(size).map_err(|_| {
                HorizonError::IpcProtocol("storage read size does not fit host usize".into())
            })?;
            let mut bytes = vec![0u8; host_size];
            storage.read_at(offset, &mut bytes).map_err(map_fs)?;
            let mut response = ServiceResponse::success();
            response.buffer_writes.push(IpcBufferWrite {
                buffer_index,
                data: bytes,
            });
            Ok(response)
        }
        storage_command::WRITE => {
            if !writable {
                return Err(HorizonError::PermissionDenied);
            }
            let offset = raw_u64_or_value(request, 0, 0)?;
            let requested = raw_u64_or_value(request, 8, 1)?;
            let data = input_buffer_bytes(request, 0)?;
            let write_len = usize::try_from(requested.min(data.len() as u64)).map_err(|_| {
                HorizonError::IpcProtocol("storage write size does not fit host usize".into())
            })?;
            storage
                .write_at(offset, &data[..write_len])
                .map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        storage_command::FLUSH => {
            storage.flush().map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        storage_command::SET_SIZE => {
            if !writable {
                return Err(HorizonError::PermissionDenied);
            }
            storage
                .set_size(raw_u64_or_value(request, 0, 0)?)
                .map_err(map_fs)?;
            Ok(ServiceResponse::success())
        }
        storage_command::GET_SIZE => {
            let mut response = ServiceResponse::success();
            response
                .values
                .push(IpcValue::U64(storage.size().map_err(map_fs)?));
            Ok(response)
        }
        id => Err(HorizonError::UnknownCommand {
            service: "IStorage".into(),
            command_id: id,
        }),
    }
}

fn input_path(request: &ServiceRequest, ordinal: usize) -> HorizonResult<GuestPath> {
    let bytes = input_buffer_bytes(request, ordinal).or_else(|_| value_bytes(request, ordinal))?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let path = std::str::from_utf8(&bytes[..end])
        .map_err(|_| HorizonError::IpcProtocol("path is not UTF-8".into()))?;
    GuestPath::parse(path).map_err(map_fs)
}

fn input_buffer_bytes(request: &ServiceRequest, ordinal: usize) -> HorizonResult<&[u8]> {
    request
        .buffers
        .iter()
        .filter(|buffer| {
            matches!(
                buffer.direction,
                IpcBufferDirection::In | IpcBufferDirection::InOut
            )
        })
        .nth(ordinal)
        .map(|buffer| buffer.data.as_slice())
        .ok_or_else(|| HorizonError::IpcProtocol(format!("missing input IPC buffer {ordinal}")))
}

fn output_buffer(request: &ServiceRequest, ordinal: usize) -> HorizonResult<(usize, u64)> {
    request
        .buffers
        .iter()
        .enumerate()
        .filter(|(_, buffer)| {
            matches!(
                buffer.direction,
                IpcBufferDirection::Out | IpcBufferDirection::InOut
            )
        })
        .nth(ordinal)
        .map(|(index, buffer)| (index, buffer.capacity))
        .ok_or_else(|| HorizonError::IpcProtocol(format!("missing output IPC buffer {ordinal}")))
}

fn value_bytes(request: &ServiceRequest, index: usize) -> HorizonResult<&[u8]> {
    match request.values.get(index) {
        Some(IpcValue::Bytes(bytes)) => Ok(bytes),
        _ => Err(HorizonError::IpcProtocol(format!(
            "argument {index} must be a byte buffer"
        ))),
    }
}

fn raw_u32(request: &ServiceRequest, offset: usize) -> HorizonResult<u32> {
    let bytes = request.raw_payload.get(offset..offset + 4).ok_or_else(|| {
        HorizonError::IpcProtocol(format!("raw u32 at offset {offset:#x} is truncated"))
    })?;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("length checked"),
    ))
}

fn raw_u64(request: &ServiceRequest, offset: usize) -> HorizonResult<u64> {
    let bytes = request.raw_payload.get(offset..offset + 8).ok_or_else(|| {
        HorizonError::IpcProtocol(format!("raw u64 at offset {offset:#x} is truncated"))
    })?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("length checked"),
    ))
}

fn raw_u32_or_value(request: &ServiceRequest, raw_offset: usize) -> HorizonResult<u32> {
    raw_u32(request, raw_offset).or_else(|_| {
        request
            .values
            .iter()
            .find_map(|value| match value {
                IpcValue::U32(value) => Some(*value),
                _ => None,
            })
            .ok_or_else(|| {
                HorizonError::IpcProtocol(format!(
                    "missing u32 argument at raw offset {raw_offset:#x}"
                ))
            })
    })
}

fn raw_u64_or_value(
    request: &ServiceRequest,
    raw_offset: usize,
    value_index: usize,
) -> HorizonResult<u64> {
    raw_u64(request, raw_offset).or_else(|_| match request.values.get(value_index) {
        Some(IpcValue::U64(value)) => Ok(*value),
        _ => Err(HorizonError::IpcProtocol(format!(
            "missing u64 argument at raw offset {raw_offset:#x}"
        ))),
    })
}

fn ensure_mount_writable(writable: bool) -> HorizonResult<()> {
    if writable {
        Ok(())
    } else {
        Err(HorizonError::PermissionDenied)
    }
}

fn map_fs(error: exosphere_vfs::FsError) -> HorizonError {
    HorizonError::FileSystem(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcBuffer;
    use exosphere_vfs::{Mount, MountId, VirtualFileSystem, backend::MemoryFileSystem};
    use std::sync::Arc;

    #[test]
    fn filesystem_open_file_and_read() {
        let backend = Arc::new(MemoryFileSystem::new(false));
        let path = GuestPath::parse("/hello.bin").unwrap();
        backend.create_file(&path, 4).unwrap();
        backend.write_file(&path, 0, b"EXO!").unwrap();
        let mut fs = crate::fs::HorizonFs::default();
        fs.mounts.mount(Mount {
            id: MountId("rom".into()),
            fs: backend,
            writable: false,
        });

        let mut object = FsSessionObject::FileSystem {
            mount: "rom".into(),
        };
        let request = ServiceRequest {
            service: "IFileSystem".into(),
            command_id: filesystem_command::OPEN_FILE,
            values: Vec::new(),
            raw_payload: 1u32.to_le_bytes().to_vec(),
            buffers: vec![IpcBuffer {
                address: 0x1000,
                capacity: 0x100,
                direction: IpcBufferDirection::In,
                data: b"/hello.bin\0".to_vec(),
            }],
        };
        let response = dispatch(&mut fs, &mut object, &request).unwrap();
        assert_eq!(response.objects.len(), 1);

        let ReturnedObject::File { mount, path, mode } =
            response.objects.into_iter().next().unwrap()
        else {
            panic!("OpenFile must return a file object");
        };
        let mut file = FsSessionObject::File {
            mount,
            path: GuestPath::parse(&path).unwrap(),
            mode: OpenMode(mode),
        };
        let read = ServiceRequest {
            service: "IFile".into(),
            command_id: file_command::READ,
            values: Vec::new(),
            raw_payload: [
                0u32.to_le_bytes().as_slice(),
                0u32.to_le_bytes().as_slice(),
                0u64.to_le_bytes().as_slice(),
                4u64.to_le_bytes().as_slice(),
            ]
            .concat(),
            buffers: vec![IpcBuffer {
                address: 0x2000,
                capacity: 4,
                direction: IpcBufferDirection::Out,
                data: Vec::new(),
            }],
        };
        let response = dispatch(&mut fs, &mut file, &read).unwrap();
        assert_eq!(response.values, vec![IpcValue::U64(4)]);
        assert_eq!(response.buffer_writes[0].data, b"EXO!");
    }
}
