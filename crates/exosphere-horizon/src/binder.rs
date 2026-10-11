// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated VI relay transactions for registered graphic producers.

use crate::{
    HorizonError, HorizonResult,
    display::DisplayManager,
    ipc::{IpcBufferDirection, ServiceRequest, ServiceResponse},
    parcel::Parcel,
};

const PRODUCER_INTERFACE: &str = "android.gui.IGraphicBufferProducer";

fn invalid(message: &str) -> HorizonError {
    HorizonError::IpcProtocol(message.into())
}

pub fn transact(
    display: &mut DisplayManager,
    owner: u64,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let args = request
        .raw_payload
        .get(..12)
        .ok_or_else(|| invalid("truncated Binder transaction arguments"))?;
    let id = i32::from_le_bytes(args[..4].try_into().unwrap());
    let transaction = u32::from_le_bytes(args[4..8].try_into().unwrap());
    let flags = u32::from_le_bytes(args[8..12].try_into().unwrap());
    // Only synchronous producer transactions are implemented. Do not silently
    // turn an asynchronous request into a synchronous state transition.
    if flags != 0 {
        return Err(invalid("unsupported Binder transaction flags"));
    }
    display.producers.get_mut(id, owner)?;
    let [input, output] = request.buffers.as_slice() else {
        return Err(invalid(
            "Binder transaction requires one input and one output buffer",
        ));
    };
    if input.direction != IpcBufferDirection::In
        || output.direction != IpcBufferDirection::Out
        || input.data.len() as u64 != input.capacity
        || output.capacity < 16
    {
        return Err(invalid(
            "invalid Binder transaction buffer directions or sizes",
        ));
    }
    let parcel = Parcel::parse(&input.data)?;
    let mut reader = parcel.reader();
    // Android's interface token begins with the caller's StrictMode policy.
    // It is metadata, not the producer identity or a graphics operation.
    let _strict_mode_policy = reader.read_u32()?;
    if reader.read_string16()? != PRODUCER_INTERFACE {
        return Err(invalid("unexpected Binder producer interface"));
    }
    if transaction == 10 {
        let listener = reader.read_u32()?;
        let api = reader.read_u32()?;
        let controlled = reader.read_u32()?;
        if listener != 0 {
            return Err(invalid("Binder producer listeners are not implemented"));
        }
        if !(1..=4).contains(&api) || controlled > 1 || reader.position() != parcel.payload.len() {
            return Err(invalid("invalid producer Connect arguments"));
        }
        const REPLY_SIZE: u64 = 16 + 20;
        if output.capacity < REPLY_SIZE {
            return Err(invalid("producer Connect output is too small"));
        }
        let producer = display.producers.get_mut(id, owner)?;
        let layer = producer.layer;
        let connected = producer.queue.is_connected();
        let pending = producer.queue.pending_count() as u32;
        let info = display
            .managed_layer(layer, owner)
            .ok_or_else(|| invalid("producer layer is detached"))?;
        // QueueBufferOutput is four u32 words followed by Android status. It
        // describes the real layer/queue, without allocating or presenting buffers.
        let words = if connected {
            [0u32, 0, 0, 0, (-22i32) as u32]
        } else {
            [info.width, info.height, 0, pending, 0]
        };
        let payload: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let bytes = crate::parcel::encode(&payload, &[])?;
        // All parsing, capacity and fallible serialization precede the mutation.
        if !connected {
            display
                .producers
                .get_mut(id, owner)?
                .connect(api, controlled != 0)?;
        }
        let mut response = ServiceResponse::success();
        response.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: 1,
            data: bytes,
        });
        return Ok(response);
    }
    // Keep unsupported producer operations explicit, with their transaction ID
    // rather than the outer relay command. No success reply or image is invented.
    Err(HorizonError::UnknownCommand {
        service: PRODUCER_INTERFACE.into(),
        command_id: transaction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcBuffer;

    fn request(id: i32, transaction: u32, arguments: &[u32]) -> ServiceRequest {
        let mut payload = 0x100u32.to_le_bytes().to_vec();
        payload.extend_from_slice(&(PRODUCER_INTERFACE.len() as u32).to_le_bytes());
        for unit in PRODUCER_INTERFACE.encode_utf16().chain([0]) {
            payload.extend_from_slice(&unit.to_le_bytes());
        }
        while payload.len() % 4 != 0 {
            payload.push(0);
        }
        for word in arguments {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        let bytes = crate::parcel::encode(&payload, &[]).unwrap();
        ServiceRequest {
            service: "IHOSBinderDriverRelay".into(),
            command_id: 0,
            values: Vec::new(),
            raw_payload: [
                id.to_le_bytes(),
                transaction.to_le_bytes(),
                0u32.to_le_bytes(),
            ]
            .concat(),
            buffers: vec![
                IpcBuffer {
                    address: 0x1000,
                    capacity: bytes.len() as u64,
                    direction: IpcBufferDirection::In,
                    data: bytes,
                },
                IpcBuffer {
                    address: 0x2000,
                    capacity: 128,
                    direction: IpcBufferDirection::Out,
                    data: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn connect_retains_api_reports_real_layer_and_rejects_before_committing() {
        for command in [0, 3] {
            let mut display = DisplayManager::default();
            let layer = display.create_managed_layer(7).unwrap();
            let window = display.open_managed_native_window(layer.id, 7).unwrap();
            let parcel = Parcel::parse(&window).unwrap();
            let id = i32::from_le_bytes(parcel.payload[8..12].try_into().unwrap());
            for arguments in [&[1u32, 2, 1][..], &[0, 0, 1], &[0, 2, 2], &[0, 2]] {
                assert!(transact(&mut display, 7, &request(id, 10, arguments)).is_err());
                assert!(
                    !display
                        .producers
                        .get_mut(id, 7)
                        .unwrap()
                        .queue
                        .is_connected()
                );
            }
            let mut request = request(id, 10, &[0, 2, 1]);
            request.command_id = command;
            request.buffers[1].capacity = 35;
            assert!(transact(&mut display, 7, &request).is_err());
            assert_eq!(display.producers.get_mut(id, 7).unwrap().connection(), None);
            request.buffers[1].capacity = 36;
            let reply = transact(&mut display, 7, &request).unwrap();
            let parcel = Parcel::parse(&reply.buffer_writes[0].data).unwrap();
            let words: Vec<u32> = parcel
                .payload
                .chunks_exact(4)
                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                .collect();
            assert_eq!(words, [layer.width, layer.height, 0, 0, 0]);
            assert_eq!(reply.buffer_writes[0].buffer_index, 1);
            let producer = display.producers.get_mut(id, 7).unwrap();
            assert_eq!(
                producer.connection(),
                Some(crate::graphic_producer::ProducerConnection {
                    api: 2,
                    controlled_by_application: true
                })
            );
            assert!(!producer.queue.availability_event().is_signaled());
            assert_eq!(producer.queue.dequeue().unwrap(), None);
            let duplicate = transact(&mut display, 7, &request).unwrap();
            let parcel = Parcel::parse(&duplicate.buffer_writes[0].data).unwrap();
            assert_eq!(&parcel.payload[16..20], &(-22i32).to_le_bytes());
            assert_eq!(
                display
                    .producers
                    .get_mut(id, 7)
                    .unwrap()
                    .connection()
                    .unwrap()
                    .api,
                2
            );
        }
    }

    #[test]
    fn transaction_checks_owner_interface_flags_and_bounded_wire_buffers() {
        let mut display = DisplayManager::default();
        let id = display.producers.create(1, 7).unwrap();
        let mut request = request(id, 9, &[]);
        assert_eq!(
            transact(&mut display, 7, &request).unwrap_err(),
            HorizonError::UnknownCommand {
                service: PRODUCER_INTERFACE.into(),
                command_id: 9
            }
        );
        assert!(matches!(
            transact(&mut display, 8, &request),
            Err(HorizonError::InvalidState(_))
        ));
        request.raw_payload[8] = 1;
        assert!(matches!(
            transact(&mut display, 7, &request),
            Err(HorizonError::IpcProtocol(_))
        ));
        request.raw_payload[8] = 0;
        request.buffers[0].data[24] = b'x';
        assert!(matches!(
            transact(&mut display, 7, &request),
            Err(HorizonError::IpcProtocol(_))
        ));
        request.buffers[0].data[24] = b'a';
        request.buffers[0].capacity += 1;
        assert!(matches!(
            transact(&mut display, 7, &request),
            Err(HorizonError::IpcProtocol(_))
        ));
        request.buffers[0].capacity -= 1;
        request.buffers[1].capacity = 15;
        assert!(matches!(
            transact(&mut display, 7, &request),
            Err(HorizonError::IpcProtocol(_))
        ));
        assert_eq!(
            display.producers.get_mut(id, 7).unwrap().reference_counts(),
            (0, 0)
        );
    }
}
