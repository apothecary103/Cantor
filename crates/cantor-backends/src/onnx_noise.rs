//! Makes diffusion noise an input of the graph.
//!
//! DiffSinger exports draw their initial noise with an unseeded
//! `RandomNormalLike` op, which ONNX Runtime seeds from the clock; renders
//! would never repeat. At load time, in memory, each such op
//! `y = RandomNormalLike(x)` is replaced by
//!
//! ```text
//! y = Reshape(Slice(cantor_noise_k, [0], Unsqueeze(Size(x), [0])), Shape(x))
//! ```
//!
//! where `cantor_noise_k` is a new 1-D float input that Cantor fills from
//! its seeded RNG. Weights and every other node pass through byte for byte.
//! See docs/decisions/0010-deterministic-diffusion-noise.md.
//!
//! This is a minimal protobuf editor for the few ONNX messages involved, so
//! no protobuf code generator is needed.

/// Prefix of the noise inputs added to rewritten graphs.
pub const NOISE_INPUT_PREFIX: &str = "cantor_noise_";

/// Errors from reading a model.
#[derive(Debug, thiserror::Error)]
#[error("malformed ONNX model: {0}")]
pub struct ProtoError(&'static str);

/// The result of a rewrite.
#[derive(Debug)]
pub struct Rewritten {
    /// The new model bytes.
    pub bytes: Vec<u8>,
    /// Names of the added noise inputs, in graph order.
    pub noise_inputs: Vec<String>,
}

// ModelProto.graph, GraphProto.{node, initializer, input},
// NodeProto.{input, output, name, op_type, attribute}.
const MODEL_GRAPH: u32 = 7;
const GRAPH_NODE: u32 = 1;
const GRAPH_INITIALIZER: u32 = 5;
const GRAPH_INPUT: u32 = 11;
const NODE_INPUT: u32 = 1;
const NODE_OUTPUT: u32 = 2;
const NODE_OP_TYPE: u32 = 4;

/// Replaces every top-level `RandomNormalLike` with a noise input. Returns
/// `None` if the model has no such op.
pub fn rewrite_random_normal(model: &[u8]) -> Result<Option<Rewritten>, ProtoError> {
    let mut out = Vec::with_capacity(model.len() + 1024);
    let mut noise_inputs = Vec::new();
    for field in Fields::new(model) {
        let field = field?;
        if field.number == MODEL_GRAPH && field.wire == WIRE_LEN {
            let graph = rewrite_graph(field.payload, &mut noise_inputs)?;
            put_len(&mut out, MODEL_GRAPH, &graph);
        } else {
            out.extend_from_slice(field.raw);
        }
    }
    Ok((!noise_inputs.is_empty()).then_some(Rewritten {
        bytes: out,
        noise_inputs,
    }))
}

fn rewrite_graph(graph: &[u8], noise_inputs: &mut Vec<String>) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::with_capacity(graph.len() + 1024);
    let mut added_inputs = Vec::new();
    let mut added_initializers = Vec::new();
    for field in Fields::new(graph) {
        let field = field?;
        if field.number == GRAPH_NODE
            && field.wire == WIRE_LEN
            && let Some((like, output)) = random_normal_like(field.payload)?
        {
            let k = noise_inputs.len();
            let noise = format!("{NOISE_INPUT_PREFIX}{k}");
            let p = format!("/cantor/noise{k}");
            let zero = format!("{p}/zero");
            for node in [
                node(
                    "Size",
                    &[&like],
                    &[&format!("{p}/size")],
                    &format!("{p}/Size"),
                ),
                node(
                    "Unsqueeze",
                    &[&format!("{p}/size"), &zero],
                    &[&format!("{p}/end")],
                    &format!("{p}/Unsqueeze"),
                ),
                node(
                    "Slice",
                    &[&noise, &zero, &format!("{p}/end")],
                    &[&format!("{p}/flat")],
                    &format!("{p}/Slice"),
                ),
                node(
                    "Shape",
                    &[&like],
                    &[&format!("{p}/shape")],
                    &format!("{p}/Shape"),
                ),
                node(
                    "Reshape",
                    &[&format!("{p}/flat"), &format!("{p}/shape")],
                    &[&output],
                    &format!("{p}/Reshape"),
                ),
            ] {
                put_len(&mut out, GRAPH_NODE, &node);
            }
            added_initializers.push(int64_tensor(&zero, &[0]));
            added_inputs.push(float_vector_input(&noise));
            noise_inputs.push(noise);
            continue;
        }
        out.extend_from_slice(field.raw);
    }
    for t in added_initializers {
        put_len(&mut out, GRAPH_INITIALIZER, &t);
    }
    for i in added_inputs {
        put_len(&mut out, GRAPH_INPUT, &i);
    }
    Ok(out)
}

/// If `node` is `RandomNormalLike`, returns its (input, output) names.
fn random_normal_like(node: &[u8]) -> Result<Option<(String, String)>, ProtoError> {
    let (mut op, mut input, mut output) = (None, None, None);
    for f in Fields::new(node) {
        let f = f?;
        match f.number {
            NODE_OP_TYPE => op = Some(f.payload),
            NODE_INPUT if input.is_none() => input = Some(f.payload),
            NODE_OUTPUT if output.is_none() => output = Some(f.payload),
            _ => {}
        }
    }
    if op != Some(b"RandomNormalLike".as_slice()) {
        return Ok(None);
    }
    let s = |b: Option<&[u8]>| {
        b.and_then(|b| std::str::from_utf8(b).ok())
            .map(str::to_owned)
            .ok_or(ProtoError("RandomNormalLike without input/output"))
    };
    Ok(Some((s(input)?, s(output)?)))
}

fn node(op: &str, inputs: &[&str], outputs: &[&str], name: &str) -> Vec<u8> {
    let mut b = Vec::new();
    for i in inputs {
        put_len(&mut b, NODE_INPUT, i.as_bytes());
    }
    for o in outputs {
        put_len(&mut b, NODE_OUTPUT, o.as_bytes());
    }
    put_len(&mut b, 3, name.as_bytes());
    put_len(&mut b, NODE_OP_TYPE, op.as_bytes());
    b
}

/// TensorProto { dims, data_type = INT64, int64_data, name }.
fn int64_tensor(name: &str, values: &[i64]) -> Vec<u8> {
    let mut b = Vec::new();
    put_varint_field(&mut b, 1, values.len() as u64);
    put_varint_field(&mut b, 2, 7);
    let mut packed = Vec::new();
    for v in values {
        put_varint(&mut packed, *v as u64);
    }
    put_len(&mut b, 7, &packed);
    put_len(&mut b, 8, name.as_bytes());
    b
}

/// ValueInfoProto for a 1-D float tensor of unknown length.
fn float_vector_input(name: &str) -> Vec<u8> {
    let mut dim = Vec::new();
    put_len(&mut dim, 2, b"noise_len"); // Dimension.dim_param
    let mut shape = Vec::new();
    put_len(&mut shape, 1, &dim); // TensorShapeProto.dim
    let mut tensor = Vec::new();
    put_varint_field(&mut tensor, 1, 1); // elem_type = FLOAT
    put_len(&mut tensor, 2, &shape);
    let mut ty = Vec::new();
    put_len(&mut ty, 1, &tensor); // TypeProto.tensor_type
    let mut b = Vec::new();
    put_len(&mut b, 1, name.as_bytes());
    put_len(&mut b, 2, &ty);
    b
}

const WIRE_VARINT: u8 = 0;
const WIRE_I64: u8 = 1;
const WIRE_LEN: u8 = 2;
const WIRE_I32: u8 = 5;

struct Field<'a> {
    number: u32,
    wire: u8,
    /// Value bytes (for length-delimited fields, without the length).
    payload: &'a [u8],
    /// The whole field including its key, for copying unchanged.
    raw: &'a [u8],
}

struct Fields<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Fields<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64, ProtoError> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self
                .buf
                .get(self.pos)
                .ok_or(ProtoError("truncated varint"))?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(ProtoError("varint too long"))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len());
        let end = end.ok_or(ProtoError("truncated field"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<Field<'a>, ProtoError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let start = self.pos;
        let res = (|| {
            let key = self.varint()?;
            let wire = (key & 7) as u8;
            let number = u32::try_from(key >> 3).map_err(|_| ProtoError("field number"))?;
            let payload = match wire {
                WIRE_VARINT => {
                    let s = self.pos;
                    self.varint()?;
                    &self.buf[s..self.pos]
                }
                WIRE_I64 => self.take(8)?,
                WIRE_LEN => {
                    let n = usize::try_from(self.varint()?).map_err(|_| ProtoError("length"))?;
                    self.take(n)?
                }
                WIRE_I32 => self.take(4)?,
                _ => return Err(ProtoError("unsupported wire type")),
            };
            Ok(Field {
                number,
                wire,
                payload,
                raw: &self.buf[start..self.pos],
            })
        })();
        if res.is_err() {
            self.pos = self.buf.len();
        }
        Some(res)
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_varint_field(out: &mut Vec<u8>, number: u32, v: u64) {
    put_varint(out, u64::from(number) << 3 | u64::from(WIRE_VARINT));
    put_varint(out, v);
}

fn put_len(out: &mut Vec<u8>, number: u32, payload: &[u8]) {
    put_varint(out, u64::from(number) << 3 | u64::from(WIRE_LEN));
    put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// Test helpers: a tiny model with one `RandomNormalLike`.
#[doc(hidden)]
pub mod test_support {
    use super::*;

    /// A model: x (float [2,3]) -> Identity -> RandomNormalLike -> y, at
    /// opset 15.
    pub fn tiny_model() -> Vec<u8> {
        let mut graph = Vec::new();
        put_len(
            &mut graph,
            GRAPH_NODE,
            &node("Identity", &["x"], &["x2"], "id"),
        );
        let mut rnl = node("RandomNormalLike", &["x2"], &["y"], "rnl");
        // attribute dtype = 1 (AttributeProto{name, i, type = INT})
        let mut attr = Vec::new();
        put_len(&mut attr, 1, b"dtype");
        put_varint_field(&mut attr, 3, 1);
        put_varint_field(&mut attr, 20, 2);
        put_len(&mut rnl, 5, &attr);
        put_len(&mut graph, GRAPH_NODE, &rnl);
        put_len(&mut graph, 2, b"tiny");
        let fixed = |name: &str| {
            let mut shape = Vec::new();
            for d in [2u64, 3] {
                let mut dim = Vec::new();
                put_varint_field(&mut dim, 1, d);
                put_len(&mut shape, 1, &dim);
            }
            let mut tensor = Vec::new();
            put_varint_field(&mut tensor, 1, 1);
            put_len(&mut tensor, 2, &shape);
            let mut ty = Vec::new();
            put_len(&mut ty, 1, &tensor);
            let mut v = Vec::new();
            put_len(&mut v, 1, name.as_bytes());
            put_len(&mut v, 2, &ty);
            v
        };
        put_len(&mut graph, GRAPH_INPUT, &fixed("x"));
        put_len(&mut graph, 12, &fixed("y")); // GraphProto.output
        let mut model = Vec::new();
        put_varint_field(&mut model, 1, 8); // ir_version
        let mut opset = Vec::new();
        put_varint_field(&mut opset, 2, 15);
        put_len(&mut model, 8, &opset); // opset_import
        put_len(&mut model, MODEL_GRAPH, &graph);
        model
    }

    /// A flow-matching velocity model for tests: inputs `x` and `cond`
    /// (float, any shape) and `t` (float [1]); output `v = cond - x`, the
    /// field whose flow relaxes `x` toward `cond`. Opset 15.
    pub fn velocity_model() -> Vec<u8> {
        let dynamic = |name: &str, rank: usize| {
            let mut shape = Vec::new();
            for d in 0..rank {
                let mut dim = Vec::new();
                put_len(&mut dim, 2, format!("{name}_{d}").as_bytes());
                put_len(&mut shape, 1, &dim);
            }
            let mut tensor = Vec::new();
            put_varint_field(&mut tensor, 1, 1);
            put_len(&mut tensor, 2, &shape);
            let mut ty = Vec::new();
            put_len(&mut ty, 1, &tensor);
            let mut v = Vec::new();
            put_len(&mut v, 1, name.as_bytes());
            put_len(&mut v, 2, &ty);
            v
        };
        let mut graph = Vec::new();
        put_len(
            &mut graph,
            GRAPH_NODE,
            &node("Sub", &["cond", "x"], &["v"], "sub"),
        );
        put_len(&mut graph, 2, b"velocity");
        put_len(&mut graph, GRAPH_INPUT, &dynamic("x", 3));
        put_len(&mut graph, GRAPH_INPUT, &dynamic("t", 1));
        put_len(&mut graph, GRAPH_INPUT, &dynamic("cond", 3));
        put_len(&mut graph, 12, &dynamic("v", 3));
        let mut model = Vec::new();
        put_varint_field(&mut model, 1, 8);
        let mut opset = Vec::new();
        put_varint_field(&mut opset, 2, 15);
        put_len(&mut model, 8, &opset);
        put_len(&mut model, MODEL_GRAPH, &graph);
        model
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::tiny_model;
    use super::*;

    #[test]
    fn rewrites_only_random_normal_like() {
        let model = tiny_model();
        let r = rewrite_random_normal(&model).unwrap().unwrap();
        assert_eq!(r.noise_inputs, ["cantor_noise_0"]);
        let text = String::from_utf8_lossy(&r.bytes);
        assert!(!text.contains("RandomNormalLike"));
        assert!(text.contains("Identity"));
        assert!(rewrite_random_normal(&r.bytes).unwrap().is_none());
    }

    #[test]
    fn rejects_truncated_input() {
        let model = tiny_model();
        assert!(rewrite_random_normal(&model[..model.len() - 3]).is_err());
    }
}
