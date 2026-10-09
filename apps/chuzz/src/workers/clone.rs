//! The graph description contains references, never buffer contents.
//! Transferred allocations are detached only after the entire graph and all
//! transfer entries have been validated. No JavaScript runs during detachment.

use std::collections::HashSet;

use blitz_script::boa_engine::builtins::array_buffer::ArrayBuffer;
use blitz_script::boa_engine::builtins::error::Error;
use blitz_script::boa_engine::object::builtins::{
    AlignedVec, JsArray, JsArrayBuffer, JsDataView, JsDate, JsMap, JsRegExp, JsSet, JsTypedArray,
};
use blitz_script::boa_engine::{
    Context, JsBigInt, JsNativeError, JsResult, JsString, JsValue, Source,
};

use super::{argument, call, named_error, register, string};

pub(super) struct Block {
    bytes: AlignedVec<u8>,
    maximum: Option<u64>,
}

pub(super) struct Wire {
    graph: String,
    buffers: Vec<Block>,
}

fn kind(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let Some(object) = argument(args, 0).as_object() else {
        return Ok(JsString::from("primitive").into());
    };
    let kind = if object.is_array() {
        "array"
    } else if object.is_ordinary() {
        "object"
    } else if JsArrayBuffer::from_object(object.clone()).is_ok() {
        "buffer"
    } else if JsTypedArray::from_object(object.clone()).is_ok() {
        "typed"
    } else if JsDataView::from_object(object.clone()).is_ok() {
        "view"
    } else if JsDate::from_object(object.clone()).is_ok() {
        "date"
    } else if JsRegExp::from_object(object.clone()).is_ok() {
        "regexp"
    } else if JsMap::from_object(object.clone()).is_ok() {
        "map"
    } else if JsSet::from_object(object.clone()).is_ok() {
        "set"
    } else if object.is::<Error>() {
        "error"
    } else if object.is::<bool>() {
        "boolean"
    } else if object.is::<f64>() {
        "number"
    } else if object.is::<JsString>() {
        "string"
    } else if object.is::<JsBigInt>() {
        "bigint"
    } else {
        "unsupported"
    };
    Ok(JsString::from(kind).into())
}

fn array(value: JsValue, context: &mut Context) -> JsResult<Vec<JsValue>> {
    let object = value
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("expected a clone array"))?;
    let array = JsArray::from_object(object)?;
    let length = array.length(context)?;
    (0..length).map(|index| array.get(index, context)).collect()
}

fn buffer(value: &JsValue, context: &mut Context) -> JsResult<JsArrayBuffer> {
    let Some(object) = value.as_object() else {
        return Err(named_error(
            context,
            "DataCloneError",
            "transfer entries must be ArrayBuffers",
        ));
    };
    JsArrayBuffer::from_object(object).map_err(|_| {
        named_error(
            context,
            "DataCloneError",
            "transfer entries must be ArrayBuffers",
        )
    })
}

fn validate(buffer: &JsArrayBuffer, context: &mut Context) -> JsResult<()> {
    let object: blitz_script::boa_engine::JsObject<ArrayBuffer> = buffer.clone().into();
    let valid = object.borrow().data().can_detach(&JsValue::undefined());
    if valid {
        Ok(())
    } else {
        Err(named_error(
            context,
            "DataCloneError",
            "buffer is detached or cannot be transferred",
        ))
    }
}

pub(super) fn encode(value: JsValue, transfers: JsValue, context: &mut Context) -> JsResult<Wire> {
    let transfers = if transfers.is_undefined() {
        JsArray::from_iter([], context).into()
    } else {
        transfers
    };
    let transfer_values = array(transfers.clone(), context)?;
    let mut transfer_objects = HashSet::new();
    for value in &transfer_values {
        let buffer = buffer(value, context)?;
        let object: blitz_script::boa_engine::JsObject = buffer.clone().into();
        if !transfer_objects.insert(object) {
            return Err(named_error(
                context,
                "DataCloneError",
                "duplicate ArrayBuffer in transfer list",
            ));
        }
        validate(&buffer, context)?;
    }

    let envelope = call(context, "__chuzzCloneEncode", &[value, transfers])?;
    let envelope = array(envelope, context)?;
    let graph = string(&envelope[0], context)?;
    let values = array(envelope[1].clone(), context)?;
    let mut buffers = Vec::with_capacity(values.len());
    let mut moves = Vec::new();

    for (index, value) in values.iter().enumerate() {
        let buffer = buffer(value, context)?;
        let object: blitz_script::boa_engine::JsObject = buffer.clone().into();
        let maximum = call(context, "__chuzzBufferMaximum", std::slice::from_ref(value))?
            .as_number()
            .filter(|maximum| *maximum >= 0.0)
            .map(|maximum| maximum as u64);
        let data = buffer.data().ok_or_else(|| {
            named_error(
                context,
                "DataCloneError",
                "cannot clone a detached ArrayBuffer",
            )
        })?;
        let bytes = if transfer_objects.contains(&object) {
            drop(data);
            validate(&buffer, context)?;
            moves.push((index, buffer));
            AlignedVec::from_iter(0, [])
        } else {
            AlignedVec::from_iter(0, data.iter().copied())
        };
        buffers.push(Block { bytes, maximum });
    }

    // A getter visited while serializing may have detached a transfer entry.
    // Recheck every entry before moving any allocation.
    for (_, buffer) in &moves {
        validate(buffer, context)?;
    }
    for (index, buffer) in moves {
        buffers[index].bytes = buffer.detach(&JsValue::undefined())?;
    }
    Ok(Wire { graph, buffers })
}

pub(super) fn decode(wire: Wire, context: &mut Context) -> JsResult<JsValue> {
    let mut buffers = Vec::with_capacity(wire.buffers.len());
    for Block { bytes, maximum } in wire.buffers {
        let mut buffer = JsArrayBuffer::from_byte_block(bytes, context)?;
        if let Some(maximum) = maximum {
            buffer = buffer.with_max_byte_length(maximum);
        }
        buffers.push(JsValue::from(buffer));
    }
    let buffers = JsArray::from_iter(buffers, context);
    call(
        context,
        "__chuzzCloneDecode",
        &[JsString::from(wire.graph).into(), buffers.into()],
    )
}

fn structured_clone(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let wire = encode(argument(args, 0), argument(args, 1), context)?;
    decode(wire, context)
}

pub(super) fn install(context: &mut Context) -> JsResult<()> {
    register(context, "__chuzzCloneKind", 1, kind)?;
    register(context, "__chuzzStructuredClone", 2, structured_clone)?;
    context.eval(Source::from_bytes(include_str!("clone.ts")))?;
    Ok(())
}
