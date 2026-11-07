use super::*;

fn staged_command(
    bridge: &Bridge,
    op: &str,
    handle: u64,
    args: Value,
    bytes: &[u8],
) -> (Command, binary::Lease) {
    let table = bridge.byte_table();
    let byte_id = table.upload(bytes).expect("upload test bytes");
    let mut args = args;
    args["data"] = json!(byte_id);
    let command = Command {
        op: op.to_owned(),
        handle,
        args,
    };
    let lease = Arc::clone(&table)
        .claim(&command)
        .expect("claim test byte upload");
    (command, lease)
}

#[test]
fn compressed_tail_mips_write_and_copy_through_webgpu_paths() {
    nagoya::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!("skipping compressed texture integration test: {error}");
                return;
            }
        };

        let candidates = [
            (
                wgpu::Features::TEXTURE_COMPRESSION_BC,
                "bc7-rgba-unorm",
                16_usize,
            ),
            (
                wgpu::Features::TEXTURE_COMPRESSION_ETC2,
                "etc2-rgb8unorm",
                8_usize,
            ),
            (
                wgpu::Features::TEXTURE_COMPRESSION_ASTC,
                "astc-4x4-unorm",
                16_usize,
            ),
        ];
        let Some((required_feature, compressed_format, bytes_per_block)) = candidates
            .into_iter()
            .find(|(feature, _, _)| adapter.features().contains(*feature))
        else {
            eprintln!(
                "skipping compressed texture integration test: adapter has no supported compression feature"
            );
            return;
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("compressed texture integration test"),
                required_features: required_feature,
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("request compressed texture test device");

        let bridge = Bridge::default();
        let report = inspector::device_report(1, &adapter, "compressed test", &device)
            .expect("describe compressed test device");
        let connection = DeviceConnection {
            id: 1,
            owner: Arc::downgrade(&bridge.0),
            device: Arc::new(device),
            queue: Arc::new(queue),
            lost: Arc::new(AtomicBool::new(false)),
            loss_reason: Arc::new(Mutex::new(None)),
            scopes: Mutex::new(Vec::new()),
            resources: Mutex::new(Resources::default()),
            inspection: Mutex::new(inspector::DeviceState::new(report)),
        };

        let texture = 10;
        let source = 11;
        let readback = 12;
        connection
            .create_texture(
                texture,
                json!({
                    "label": "compressed tail texture",
                    "size": [4, 4, 1],
                    "mipLevelCount": 3,
                    "sampleCount": 1,
                    "dimension": "2d",
                    "format": compressed_format,
                    "usage": 3,
                    "viewFormats": []
                }),
                None,
            )
            .await
            .expect("create compressed tail texture");

        let expected: Vec<u8> = (0..bytes_per_block)
            .map(|byte| (byte as u8).wrapping_mul(13).wrapping_add(7))
            .collect();
        let mut source_bytes = vec![0_u8; 256];
        source_bytes[..bytes_per_block].copy_from_slice(&expected);

        // Queue writes use a compact block row and cover the 2x2 mip logically.
        let (write, write_lease) = staged_command(
            &bridge,
            "writeTexture",
            0,
            json!({
                "texture": texture,
                "mipLevel": 1,
                "origin": [0, 0, 0],
                "aspect": "all",
                "offset": 0,
                "bytesPerRow": bytes_per_block as u32,
                "rowsPerImage": 1,
                "size": [2, 2, 1]
            }),
            &expected,
        );
        connection
            .execute(write)
            .await
            .expect("queue.writeTexture tail mip");
        drop(write_lease);

        // The encoder copy uses 256-byte row alignment for a GPU buffer source.
        connection
            .create_buffer(
                source,
                json!({
                    "label": "compressed copy source",
                    "size": source_bytes.len(),
                    "usage": 12,
                    "mappedAtCreation": false
                }),
            )
            .await
            .expect("create compressed copy source");
        let (write_buffer, buffer_lease) = staged_command(
            &bridge,
            "writeBuffer",
            source,
            json!({"offset": 0}),
            &source_bytes,
        );
        connection
            .execute_resource(write_buffer)
            .await
            .expect("upload compressed copy source");
        drop(buffer_lease);

        connection
            .execute_encoding(
                "createCommandEncoder",
                20,
                json!({"label": "compressed copy"}),
            )
            .await
            .expect("create copy encoder");
        connection
            .execute_encoding(
                "finish",
                21,
                json!({
                    "encoder": 20,
                    "label": "compressed copy commands",
                    "operations": [{
                        "op": "copyBufferToTexture",
                        "args": {
                            "source": {
                                "buffer": source,
                                "offset": 0,
                                "bytesPerRow": 256,
                                "rowsPerImage": 1
                            },
                            "destination": {
                                "texture": texture,
                                "mipLevel": 2,
                                "origin": [0, 0, 0],
                                "aspect": "all"
                            },
                            "size": [1, 1, 1]
                        }
                    }]
                }),
            )
            .await
            .expect("encode copyBufferToTexture tail mip");
        connection
            .execute_encoding("submit", 21, json!([21]))
            .await
            .expect("submit compressed copy");

        connection
            .create_buffer(
                readback,
                json!({
                    "label": "compressed texture readback",
                    "size": 512,
                    "usage": 9,
                    "mappedAtCreation": false
                }),
            )
            .await
            .expect("create compressed readback buffer");
        connection
            .execute_encoding(
                "createCommandEncoder",
                30,
                json!({"label": "compressed readback"}),
            )
            .await
            .expect("create readback encoder");
        connection
            .execute_encoding(
                "finish",
                31,
                json!({
                    "encoder": 30,
                    "label": "compressed readback commands",
                    "operations": ([1, 2].into_iter().enumerate().map(|(index, mip)| json!({
                        "op": "copyTextureToBuffer",
                        "args": {
                            "source": {
                                "texture": texture,
                                "mipLevel": mip,
                                "origin": [0, 0, 0],
                                "aspect": "all"
                            },
                            "destination": {
                                "buffer": readback,
                                "offset": index * 256,
                                "bytesPerRow": 256,
                                "rowsPerImage": 1
                            },
                            "size": if mip == 1 { [2, 2, 1] } else { [1, 1, 1] }
                        }
                    })).collect::<Vec<_>>())
                }),
            )
            .await
            .expect("encode compressed texture readback");
        connection
            .execute_encoding("submit", 31, json!([31]))
            .await
            .expect("submit compressed texture readback");

        let mapped = connection
            .map_buffer(readback, json!({"offset": 0, "size": 512}))
            .await
            .expect("map compressed texture readback");
        let byte_id = mapped["byteId"].as_u64().expect("readback byte handle");
        let binary::Block::Buffer(actual) = bridge
            .byte_table()
            .take_output(byte_id)
            .expect("take readback bytes")
        else {
            panic!("expected byte buffer readback");
        };
        assert_eq!(&actual[..bytes_per_block], expected.as_slice());
        assert_eq!(&actual[256..256 + bytes_per_block], expected.as_slice());
    });
}
