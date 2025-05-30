// Copyright Materialize, Inc. and contributors. All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use std::cmp;
use std::io::{BufRead, Read};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use byteorder::{NetworkEndian, WriteBytesExt};
use futures::stream::{FuturesUnordered, StreamExt};
use maplit::btreemap;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::FutureRecord;
use serde::de::DeserializeOwned;
use tokio::fs;

use crate::action::{self, ControlFlow, State};
use crate::format::avro::{self, Schema};
use crate::format::bytes;
use crate::parser::BuiltinCommand;

const INGEST_BATCH_SIZE: isize = 10000;

#[derive(Clone)]
enum Format {
    Avro {
        schema: String,
        confluent_wire_format: bool,
    },
    Protobuf {
        descriptor_file: String,
        message: String,
        confluent_wire_format: bool,
        schema_id_subject: Option<String>,
        schema_message_id: u8,
    },
    Bytes {
        terminator: Option<u8>,
    },
}

enum Transcoder {
    PlainAvro {
        schema: Schema,
    },
    ConfluentAvro {
        schema: Schema,
        schema_id: i32,
    },
    Protobuf {
        message: MessageDescriptor,
        confluent_wire_format: bool,
        schema_id: i32,
        schema_message_id: u8,
    },
    Bytes {
        terminator: Option<u8>,
    },
}

impl Transcoder {
    fn decode_json<R, T>(row: R) -> Result<Option<T>, anyhow::Error>
    where
        R: Read,
        T: DeserializeOwned,
    {
        let deserializer = serde_json::Deserializer::from_reader(row);
        deserializer
            .into_iter()
            .next()
            .transpose()
            .context("parsing json")
    }

    fn transcode<R>(&self, mut row: R) -> Result<Option<Vec<u8>>, anyhow::Error>
    where
        R: BufRead,
    {
        match self {
            Transcoder::ConfluentAvro { schema, schema_id } => {
                if let Some(val) = Self::decode_json(row)? {
                    let val = avro::from_json(&val, schema.top_node())?;
                    let mut out = vec![];
                    // The first byte is a magic byte (0) that indicates the Confluent
                    // serialization format version, and the next four bytes are a
                    // 32-bit schema ID.
                    //
                    // https://docs.confluent.io/3.3.0/schema-registry/docs/serializer-formatter.html#wire-format
                    out.write_u8(0).unwrap();
                    out.write_i32::<NetworkEndian>(*schema_id).unwrap();
                    out.extend(avro::to_avro_datum(schema, val)?);
                    Ok(Some(out))
                } else {
                    Ok(None)
                }
            }
            Transcoder::PlainAvro { schema } => {
                if let Some(val) = Self::decode_json(row)? {
                    let val = avro::from_json(&val, schema.top_node())?;
                    let mut out = vec![];
                    out.extend(avro::to_avro_datum(schema, val)?);
                    Ok(Some(out))
                } else {
                    Ok(None)
                }
            }
            Transcoder::Protobuf {
                message,
                confluent_wire_format,
                schema_id,
                schema_message_id,
            } => {
                if let Some(val) = Self::decode_json::<_, serde_json::Value>(row)? {
                    let message = DynamicMessage::deserialize(message.clone(), val)
                        .context("parsing protobuf JSON")?;
                    let mut out = vec![];
                    if *confluent_wire_format {
                        // See: https://github.com/MaterializeInc/database-issues/issues/2837
                        // The first byte is a magic byte (0) that indicates the Confluent
                        // serialization format version, and the next four bytes are a
                        // 32-bit schema ID, which we default to something fun.
                        // And, as we only support single-message proto files for now,
                        // we also set the following message id to 0.
                        out.write_u8(0).unwrap();
                        out.write_i32::<NetworkEndian>(*schema_id).unwrap();
                        out.write_u8(*schema_message_id).unwrap();
                    }
                    message.encode(&mut out)?;
                    Ok(Some(out))
                } else {
                    Ok(None)
                }
            }
            Transcoder::Bytes { terminator } => {
                let mut out = vec![];
                match terminator {
                    Some(t) => {
                        row.read_until(*t, &mut out)?;
                        if out.last() == Some(t) {
                            out.pop();
                        }
                    }
                    None => {
                        row.read_to_end(&mut out)?;
                    }
                }
                if out.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(bytes::unescape(&out)?))
                }
            }
        }
    }
}

pub async fn run_ingest(
    mut cmd: BuiltinCommand,
    state: &mut State,
) -> Result<ControlFlow, anyhow::Error> {
    let topic_prefix = format!("testdrive-{}", cmd.args.string("topic")?);
    let partition = cmd.args.opt_parse::<i32>("partition")?;
    let start_iteration = cmd.args.opt_parse::<isize>("start-iteration")?.unwrap_or(0);
    let repeat = cmd.args.opt_parse::<isize>("repeat")?.unwrap_or(1);
    let omit_key = cmd.args.opt_bool("omit-key")?.unwrap_or(false);
    let omit_value = cmd.args.opt_bool("omit-value")?.unwrap_or(false);
    let schema_id_var = cmd.args.opt_parse("set-schema-id-var")?;
    let key_schema_id_var = cmd.args.opt_parse("set-key-schema-id-var")?;
    let format = match cmd.args.string("format")?.as_str() {
        "avro" => Format::Avro {
            schema: cmd.args.string("schema")?,
            confluent_wire_format: cmd.args.opt_bool("confluent-wire-format")?.unwrap_or(true),
        },
        "protobuf" => {
            let descriptor_file = cmd.args.string("descriptor-file")?;
            let message = cmd.args.string("message")?;
            Format::Protobuf {
                descriptor_file,
                message,
                // This was introduced after the avro format's confluent-wire-format, so it defaults to
                // false
                confluent_wire_format: cmd.args.opt_bool("confluent-wire-format")?.unwrap_or(false),
                schema_id_subject: cmd.args.opt_string("schema-id-subject"),
                schema_message_id: cmd.args.opt_parse::<u8>("schema-message-id")?.unwrap_or(0),
            }
        }
        "bytes" => Format::Bytes { terminator: None },
        f => bail!("unknown format: {}", f),
    };
    let mut key_schema = cmd.args.opt_string("key-schema");
    let key_format = match cmd.args.opt_string("key-format").as_deref() {
        Some("avro") => Some(Format::Avro {
            schema: key_schema.take().ok_or_else(|| {
                anyhow!("key-schema parameter required when key-format is present")
            })?,
            confluent_wire_format: cmd.args.opt_bool("confluent-wire-format")?.unwrap_or(true),
        }),
        Some("protobuf") => {
            let descriptor_file = cmd.args.string("key-descriptor-file")?;
            let message = cmd.args.string("key-message")?;
            Some(Format::Protobuf {
                descriptor_file,
                message,
                confluent_wire_format: cmd.args.opt_bool("confluent-wire-format")?.unwrap_or(false),
                schema_id_subject: cmd.args.opt_string("key-schema-id-subject"),
                schema_message_id: cmd
                    .args
                    .opt_parse::<u8>("key-schema-message-id")?
                    .unwrap_or(0),
            })
        }
        Some("bytes") => Some(Format::Bytes {
            terminator: match cmd.args.opt_parse::<char>("key-terminator")? {
                Some(c) => match u8::try_from(c) {
                    Ok(c) => Some(c),
                    Err(_) => bail!("key terminator must be single ASCII character"),
                },
                None => Some(b':'),
            },
        }),
        Some(f) => bail!("unknown key format: {}", f),
        None => None,
    };
    if key_schema.is_some() {
        anyhow::bail!("key-schema specified without a matching key-format");
    }

    let timestamp = cmd.args.opt_parse("timestamp")?;

    use serde_json::Value;
    let headers = if let Some(headers_val) = cmd.args.opt_parse::<serde_json::Value>("headers")? {
        let mut headers = Vec::new();
        let headers_maps = match headers_val {
            Value::Array(values) => {
                let mut headers_map = Vec::new();
                for value in values {
                    if let Value::Object(m) = value {
                        headers_map.push(m)
                    } else {
                        bail!("`headers` array values must be maps")
                    }
                }
                headers_map
            }
            Value::Object(v) => vec![v],
            _ => bail!("`headers` must be a map or an array"),
        };

        for headers_map in headers_maps {
            for (k, v) in headers_map.iter() {
                headers.push((k.clone(), match v {
                    Value::String(val) => Some(val.as_bytes().to_vec()),
                    Value::Array(val) => {
                        let mut values = Vec::new();
                        for value in val {
                            if let Value::Number(int) = value {
                                values.push(u8::try_from(int.as_i64().unwrap()).unwrap())
                            } else {
                                bail!("`headers` value arrays must only contain numbers (to represent bytes)")
                            }
                        }
                        Some(values.clone())
                    },
                    Value::Null => None,
                    _ => bail!("`headers` must have string, int array or null values")
                }));
            }
        }
        Some(headers)
    } else {
        None
    };

    cmd.args.done()?;

    if let Some(kf) = &key_format {
        fn is_confluent_format(fmt: &Format) -> Option<bool> {
            match fmt {
                Format::Avro {
                    confluent_wire_format,
                    ..
                } => Some(*confluent_wire_format),
                Format::Protobuf {
                    confluent_wire_format,
                    ..
                } => Some(*confluent_wire_format),
                Format::Bytes { .. } => None,
            }
        }
        match (is_confluent_format(kf), is_confluent_format(&format)) {
            (Some(false), Some(true)) | (Some(true), Some(false)) => {
                bail!(
                    "It does not make sense to have the key be in confluent format and not the value, or vice versa."
                );
            }
            _ => {}
        }
    }

    let topic_name = &format!("{}-{}", topic_prefix, state.seed);
    println!(
        "Ingesting data into Kafka topic {} with start_iteration = {}, repeat = {}",
        topic_name, start_iteration, repeat
    );

    let set_schema_id_var = |state: &mut State, schema_id_var, transcoder| match transcoder {
        &Transcoder::ConfluentAvro { schema_id, .. } | &Transcoder::Protobuf { schema_id, .. } => {
            state.cmd_vars.insert(schema_id_var, schema_id.to_string());
        }
        _ => (),
    };

    let value_transcoder =
        make_transcoder(state, format.clone(), format!("{}-value", topic_name)).await?;
    if let Some(var) = schema_id_var {
        set_schema_id_var(state, var, &value_transcoder);
    }

    let key_transcoder = match key_format.clone() {
        None => None,
        Some(f) => {
            let transcoder = make_transcoder(state, f, format!("{}-key", topic_name)).await?;
            if let Some(var) = key_schema_id_var {
                set_schema_id_var(state, var, &transcoder);
            }
            Some(transcoder)
        }
    };

    let mut futs = FuturesUnordered::new();

    for iteration in start_iteration..(start_iteration + repeat) {
        let iter = &mut cmd.input.iter().peekable();

        for row in iter {
            let row = action::substitute_vars(
                row,
                &btreemap! { "kafka-ingest.iteration".into() => iteration.to_string() },
                &None,
                false,
            )?;
            let mut row = row.as_bytes();
            let key = match (omit_key, &key_transcoder) {
                (true, _) => None,
                (false, None) => None,
                (false, Some(kt)) => kt.transcode(&mut row)?,
            };
            let value = if omit_value {
                None
            } else {
                value_transcoder
                    .transcode(&mut row)
                    .with_context(|| format!("parsing row: {}", String::from_utf8_lossy(row)))?
            };
            let producer = &state.kafka_producer;
            let timeout = cmp::max(state.default_timeout, Duration::from_secs(1));
            let headers = headers.clone();
            futs.push(async move {
                let mut record: FutureRecord<_, _> = FutureRecord::to(topic_name);

                if let Some(partition) = partition {
                    record = record.partition(partition);
                }
                if let Some(key) = &key {
                    record = record.key(key);
                }
                if let Some(value) = &value {
                    record = record.payload(value);
                }
                if let Some(timestamp) = timestamp {
                    record = record.timestamp(timestamp);
                }
                if let Some(headers) = headers {
                    let mut rd_meta = OwnedHeaders::new();
                    for (k, v) in &headers {
                        rd_meta = rd_meta.insert(Header {
                            key: k,
                            value: v.as_deref(),
                        });
                    }
                    record = record.headers(rd_meta);
                }
                producer.send(record, timeout).await
            });
        }

        // Reap the futures thus produced periodically or after the last iteration
        if iteration % INGEST_BATCH_SIZE == 0 || iteration == (start_iteration + repeat - 1) {
            while let Some(res) = futs.next().await {
                res.map_err(|(e, _message)| e)?;
            }
        }
    }
    Ok(ControlFlow::Continue)
}

async fn make_transcoder(
    state: &State,
    format: Format,
    ccsr_subject: String,
) -> Result<Transcoder, anyhow::Error> {
    match format {
        Format::Avro {
            schema,
            confluent_wire_format,
        } => {
            if confluent_wire_format {
                let schema_id = state
                    .ccsr_client
                    .publish_schema(&ccsr_subject, &schema, mz_ccsr::SchemaType::Avro, &[])
                    .await
                    .context("publishing to schema registry")?;
                let schema = avro::parse_schema(&schema)
                    .with_context(|| format!("parsing avro schema: {}", schema))?;
                Ok::<_, anyhow::Error>(Transcoder::ConfluentAvro { schema, schema_id })
            } else {
                let schema = avro::parse_schema(&schema)
                    .with_context(|| format!("parsing avro schema: {}", schema))?;
                Ok(Transcoder::PlainAvro { schema })
            }
        }
        Format::Protobuf {
            descriptor_file,
            message,
            confluent_wire_format,
            schema_id_subject,
            schema_message_id,
        } => {
            let schema_id = if confluent_wire_format {
                state
                    .ccsr_client
                    .get_schema_by_subject(schema_id_subject.as_deref().unwrap_or(&ccsr_subject))
                    .await
                    .context("fetching schema from registry")?
                    .id
            } else {
                0
            };

            let bytes = fs::read(state.temp_path.join(descriptor_file))
                .await
                .context("reading protobuf descriptor file")?;
            let fd = DescriptorPool::decode(&*bytes).context("parsing protobuf descriptor file")?;
            let message = fd
                .get_message_by_name(&message)
                .ok_or_else(|| anyhow!("unknown message name {}", message))?;
            Ok(Transcoder::Protobuf {
                message,
                confluent_wire_format,
                schema_id,
                schema_message_id,
            })
        }
        Format::Bytes { terminator } => Ok(Transcoder::Bytes { terminator }),
    }
}
