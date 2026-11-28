//! Best-effort extraction of YouTube's classic JavaScript signature transforms.
//!
//! This is deliberately **not** a JavaScript parser and not an EJS-grade solver.
//! The supported input is the small pre-2020-style transform shape:
//!
//! - one function taking the challenge string,
//! - `split("")`, a sequence of reverse/splice/swap array mutations, then
//!   `join("")`,
//! - mutations either direct (`a.reverse()`, `a.splice(0, b)`) or delegated to a
//!   small object whose methods have the classic reverse/splice/swap bodies,
//! - no loops, conditionals, nested functions, DOM access, or large obfuscated
//!   opcode arrays.
//!
//! The extracted function and helper object are executed in QuickJS. They are
//! never merged back into the whole player program, so modern `base.js` builds
//! that require lexical scope, arrays of generated functions, or control flow
//! fail explicitly as unsupported instead of producing a wrong signature.
//!
//! # Sandbox model
//!
//! The player JavaScript is attacker-controlled input, so execution is bounded
//! in every dimension QuickJS exposes:
//!
//! - a fresh `Runtime`+`Context` per `apply` call, so prototype pollution can
//!   never leak into a later decipher job,
//! - a memory limit, a stack limit, an instruction-count interrupt budget and
//!   a wall-clock deadline (the count budget alone is not a time bound: the
//!   interpreter only polls the handler every few thousand bytecode events, so
//!   a loop with a heavy body could run for hours within the same count),
//! - a hardening preamble that deletes the `eval`/`Function` globals, severs
//!   the constructor chains that reach the code-compiling constructors
//!   (`obj.constructor.constructor`, generator/async variants) and freezes the
//!   standard intrinsics before any player-derived code runs,
//! - no host bridge of any kind: no module loader, no `std`/`os`, no print
//!   callback, so there is no exfiltration channel even if every other layer
//!   fails.
//!
//! The `Promise` intrinsic is enabled deliberately even though transforms
//! never use it: without it, creating an async function object leaves a
//! half-initialized object behind and QuickJS aborts the process in
//! `JS_FreeRuntime` (its assertions are compiled in). Microtasks never run
//! because nothing drives the job queue of a synchronous runtime.

use rquickjs::{context::intrinsic, Context, Runtime};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const MAX_TRANSFORM_SOURCE: usize = 8 * 1024;
const MAX_HELPER_SOURCE: usize = 8 * 1024;
const MAX_INPUT_CHARS: usize = 2048;
const JS_MEMORY_LIMIT: usize = 4 * 1024 * 1024;
const JS_STACK_LIMIT: usize = 256 * 1024;
const JS_INTERRUPT_BUDGET: usize = 20_000;
/// Hard per-application wall-clock limit for the whole sandboxed evaluation.
/// Legitimate classic transforms finish in microseconds, so this is several
/// orders of magnitude of slack and only ever trips on hostile or degenerate
/// input.
const JS_WALL_CLOCK_LIMIT: Duration = Duration::from_secs(5);

/// SES-style lockdown executed before any player-derived code. It must stay
/// dependency-free sloppy-compatible ES5 (it runs ahead of unvalidated input)
/// with one exception: the IIFE body itself is strict.
///
/// Steps: drop the global `eval`/`Function` bindings, redefine every own
/// `constructor` property on the intrinsics below (most importantly
/// `Function.prototype.constructor`, the terminal hop of every
/// `x.constructor.constructor` chain, and the hidden generator/async
/// prototypes reached only through instances) to a non-configurable
/// `undefined`, freeze the standard intrinsics and the global object, then
/// probe the escapes behaviorally and refuse to run if any survived. The
/// probes make this fail-closed: if a future QuickJS build adds another
/// reachable code-compiling constructor, deciphering errors out instead of
/// executing attacker-controlled compilation.
const HARDENING_PREAMBLE: &str = r#"
;(function () {
  "use strict";
  var g = globalThis;
  var defineProperty = Object.defineProperty;
  var getPrototypeOf = Object.getPrototypeOf;
  var freeze = Object.freeze;
  // Capture before the global bindings are dropped.
  var fnProto = getPrototypeOf(function () {});

  delete g.eval;
  delete g.Function;

  function neuter(obj) {
    if (!obj) { return; }
    try {
      defineProperty(obj, "constructor", { value: undefined, writable: false, configurable: false });
    } catch (e) {}
  }
  var hiddenProtos = [];
  try { hiddenProtos.push(getPrototypeOf(function*(){})); } catch (e) {}
  try { hiddenProtos.push(getPrototypeOf(async function(){})); } catch (e) {}
  try { hiddenProtos.push(getPrototypeOf(async function*(){})); } catch (e) {}

  var names = [
    "Object", "Array", "String", "Number", "Boolean", "BigInt", "Symbol",
    "Math", "Reflect", "Error", "AggregateError", "EvalError", "InternalError",
    "RangeError", "ReferenceError", "SuppressedError", "SyntaxError", "TypeError",
    "URIError", "Iterator", "DisposableStack", "Promise"
  ];
  var hardenList = [g, fnProto, Object.prototype];
  var i;
  for (i = 0; i < hiddenProtos.length; i++) { hardenList.push(hiddenProtos[i]); }
  for (i = 0; i < names.length; i++) {
    var ctor = g[names[i]];
    if ((typeof ctor === "object" && ctor !== null) || typeof ctor === "function") {
      hardenList.push(ctor);
      if ((typeof ctor.prototype === "object" && ctor.prototype !== null) || typeof ctor.prototype === "function") {
        hardenList.push(ctor.prototype);
      }
    }
  }
  for (i = 0; i < hardenList.length; i++) {
    neuter(hardenList[i]);
  }
  for (i = 0; i < hardenList.length; i++) {
    try { freeze(hardenList[i]); } catch (e) {}
  }

  var failures = [];
  function mustThrow(fn, label) {
    var threw = false;
    try { fn(); } catch (e) { threw = true; }
    if (!threw) { failures.push(label); }
  }
  if (typeof g.eval !== "undefined") { failures.push("eval-global"); }
  if (typeof g.Function !== "undefined") { failures.push("Function-global"); }
  mustThrow(function () { return g.eval("1"); }, "eval-indirect");
  mustThrow(function () { return ({}).constructor.constructor("return 1")(); }, "object-constructor-chain");
  mustThrow(function () { return (function*(){}).constructor("yield 1")(); }, "generator-constructor-chain");
  mustThrow(function () { return (async function(){}).constructor("return 1")(); }, "async-constructor-chain");
  mustThrow(function () { return (async function*(){}).constructor("yield 1")(); }, "async-generator-constructor-chain");
  if (typeof (function(){}).constructor !== "undefined") { failures.push("function-prototype-constructor"); }
  if (failures.length) {
    throw new Error("cipher sandbox hardening incomplete: " + failures.join(","));
  }
})();
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CipherError {
    /// The player JavaScript does not match the documented classic shapes.
    Unsupported(String),
    /// The shape was extracted but QuickJS could not execute it.
    Failed(String),
}

impl CipherError {
    fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported(reason.into())
    }

    fn failed(reason: impl Into<String>) -> Self {
        Self::Failed(reason.into())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TransformProgram {
    function_source: String,
    helpers: Vec<(String, String)>,
}

impl TransformProgram {
    /// Run the extracted transform in a small, memory-, instruction- and
    /// time-limited QuickJS instance. No host objects, module loader, file
    /// system or network are exposed to the script, and the sandbox intrinsics
    /// are locked down before any player-derived code runs (see the module
    /// documentation).
    pub(crate) fn apply(&self, input: &str) -> Result<String, CipherError> {
        self.apply_with_limits(input, JS_WALL_CLOCK_LIMIT, JS_INTERRUPT_BUDGET)
    }

    fn apply_with_limits(
        &self,
        input: &str,
        wall_clock: Duration,
        interrupt_budget: usize,
    ) -> Result<String, CipherError> {
        if input.chars().count() > MAX_INPUT_CHARS {
            return Err(CipherError::unsupported("challenge input is unexpectedly long"));
        }
        let runtime =
            Runtime::new().map_err(|e| CipherError::failed(format!("QuickJS runtime: {e}")))?;
        runtime.set_memory_limit(JS_MEMORY_LIMIT);
        runtime.set_max_stack_size(JS_STACK_LIMIT);
        // The handler is only polled every few thousand bytecode events, so
        // the count budget cannot bound wall time on its own for loops with a
        // heavy body; the deadline is the primary termination guarantee and
        // the count budget is a clock-independent backstop.
        let deadline = Instant::now() + wall_clock;
        let mut interrupts = 0usize;
        runtime.set_interrupt_handler(Some(Box::new(move || {
            interrupts = interrupts.saturating_add(1);
            interrupts > interrupt_budget || Instant::now() >= deadline
        })));
        // `Eval` is the minimum intrinsic set needed by QuickJS for ordinary
        // function calls/evaluation. It adds no host I/O capability. `Promise`
        // is enabled only because async function objects created without it
        // are half-initialized and abort the process when the runtime is
        // freed; nothing ever drives the microtask queue, so promises stay
        // inert.
        let context = Context::builder()
            .with::<intrinsic::Eval>()
            .with::<intrinsic::Promise>()
            .build(&runtime)
            .map_err(|e| CipherError::failed(format!("QuickJS context: {e}")))?;
        let input_json = serde_json::to_string(input)
            .map_err(|e| CipherError::failed(format!("challenge encoding failed: {e}")))?;
        // The hardening preamble must precede all player-derived code in the
        // same script. Helpers and the transform live inside an IIFE so they
        // never touch the (frozen) global object; classic transforms resolve
        // their helper objects through the closure exactly as they would
        // through globals.
        let mut script = String::new();
        script.push_str(HARDENING_PREAMBLE);
        script.push_str("\n(function(){\n");
        for (name, object) in &self.helpers {
            script.push_str("var ");
            script.push_str(name);
            script.push_str(" = ");
            script.push_str(object);
            script.push_str(";\n");
        }
        script.push_str("var __falcon_transform = (");
        script.push_str(&self.function_source);
        script.push_str(");\nreturn String(__falcon_transform(");
        script.push_str(&input_json);
        script.push_str("));\n})()");
        context
            .with(|ctx| ctx.eval::<String, _>(script))
            .map_err(|e| CipherError::failed(format!("transform execution failed: {e}")))
    }
}

#[derive(Clone, Debug)]
struct FunctionHeader {
    start: usize,
    declared_name: Option<String>,
    params: String,
    body_open: usize,
}

#[derive(Clone, Debug)]
struct FunctionSource {
    source: String,
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

fn is_ident_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn is_identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    matches!(bytes.next(), Some(first) if is_ident_start(first)) && bytes.all(is_ident_char)
}

fn skip_ws(bytes: &[u8], mut index: usize) -> usize {
    while matches!(bytes.get(index), Some(byte) if byte.is_ascii_whitespace()) {
        index += 1;
    }
    index
}

fn parse_identifier(bytes: &[u8], index: usize) -> Option<(String, usize)> {
    let first = *bytes.get(index)?;
    if !is_ident_start(first) {
        return None;
    }
    let mut end = index + 1;
    while matches!(bytes.get(end), Some(byte) if is_ident_char(*byte)) {
        end += 1;
    }
    std::str::from_utf8(&bytes[index..end]).ok().map(str::to_string).map(|s| (s, end))
}

fn identifier_boundaries(bytes: &[u8], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !is_ident_char(bytes[start - 1]);
    let after = start + len;
    let after_ok = after >= bytes.len() || !is_ident_char(bytes[after]);
    before_ok && after_ok
}

/// Match a delimiter while ignoring quoted strings and comments. Regex
/// literals and template substitutions are deliberately not modeled; shapes
/// requiring that level of JavaScript parsing are outside this solver's scope.
fn find_matching(source: &str, open_pos: usize, open: u8, close: u8) -> Option<usize> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment,
    }

    let bytes = source.as_bytes();
    let mut state = State::Normal;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut i = open_pos;
    while i < bytes.len() {
        let byte = bytes[i];
        match state {
            State::LineComment => {
                if byte == b'\n' || byte == b'\r' {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                if byte == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    state = State::Normal;
                    i += 1;
                }
            }
            State::Single | State::Double | State::Template => {
                let quote = match state {
                    State::Single => b'\'',
                    State::Double => b'"',
                    _ => b'`',
                };
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == quote {
                    state = State::Normal;
                }
            }
            State::Normal => {
                if byte == b'/' && bytes.get(i + 1) == Some(&b'/') {
                    state = State::LineComment;
                    i += 1;
                } else if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    state = State::BlockComment;
                    i += 1;
                } else if byte == b'\'' {
                    state = State::Single;
                } else if byte == b'"' {
                    state = State::Double;
                } else if byte == b'`' {
                    state = State::Template;
                } else if byte == open {
                    depth += 1;
                } else if byte == close {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn parse_function_header(source: &str, start: usize) -> Option<FunctionHeader> {
    let bytes = source.as_bytes();
    if !source.get(start..)?.starts_with("function") {
        return None;
    }
    let mut index = skip_ws(bytes, start + "function".len());
    let mut declared_name = None;
    if matches!(bytes.get(index), Some(byte) if is_ident_start(*byte)) {
        let (name, end) = parse_identifier(bytes, index)?;
        declared_name = Some(name);
        index = skip_ws(bytes, end);
    }
    if bytes.get(index) != Some(&b'(') {
        return None;
    }
    let params_end = find_matching(source, index, b'(', b')')?;
    let params = source.get(index + 1..params_end)?.to_string();
    index = skip_ws(bytes, params_end + 1);
    if bytes.get(index) != Some(&b'{') {
        return None;
    }
    Some(FunctionHeader { start, declared_name, params, body_open: index })
}

fn collect_function_headers(js: &str) -> Vec<FunctionHeader> {
    let bytes = js.as_bytes();
    let mut headers = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = js.get(offset..).and_then(|tail| tail.find("function")) {
        let start = offset + relative;
        if identifier_boundaries(bytes, start, "function".len()) {
            if let Some(header) = parse_function_header(js, start) {
                headers.push(header);
            }
        }
        offset = start.saturating_add("function".len());
    }
    headers
}

fn assigned_name_before(js: &str, header: &FunctionHeader) -> Option<String> {
    if let Some(name) = &header.declared_name {
        return Some(name.clone());
    }
    let bytes = js.as_bytes();
    let mut index = header.start;
    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }
    if index == 0 || bytes[index - 1] != b'=' {
        return None;
    }
    index -= 1;
    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }
    let end = index;
    while index > 0 && is_ident_char(bytes[index - 1]) {
        index -= 1;
    }
    if index == end || (end < bytes.len() && is_ident_char(bytes[end])) {
        return None;
    }
    let name = std::str::from_utf8(&bytes[index..end]).ok()?;
    is_identifier(name).then(|| name.to_string())
}

fn function_source(js: &str, header: &FunctionHeader) -> Option<FunctionSource> {
    let close = find_matching(js, header.body_open, b'{', b'}')?;
    Some(FunctionSource { source: js.get(header.start..=close)?.to_string() })
}

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

fn contains_word(source: &str, word: &str) -> bool {
    let bytes = source.as_bytes();
    source.match_indices(word).any(|(index, _)| identifier_boundaries(bytes, index, word.len()))
}

fn member_calls(body: &str) -> Vec<(String, String)> {
    let bytes = body.as_bytes();
    let mut calls = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if !is_ident_start(bytes[index]) {
            index += 1;
            continue;
        }
        let (object, mut cursor) = match parse_identifier(bytes, index) {
            Some(parsed) => parsed,
            None => {
                index += 1;
                continue;
            }
        };
        cursor = skip_ws(bytes, cursor);
        if bytes.get(cursor) != Some(&b'.') {
            index = cursor.max(index + 1);
            continue;
        }
        cursor = skip_ws(bytes, cursor + 1);
        let Some((method, after_method)) = parse_identifier(bytes, cursor) else {
            index = cursor.max(index + 1);
            continue;
        };
        let after_ws = skip_ws(bytes, after_method);
        if bytes.get(after_ws) == Some(&b'(') {
            calls.push((object, method));
        }
        index = after_method.max(index + 1);
    }
    calls
}

fn validate_helper_method(params: &str, body: &str, method_name: &str) -> Result<(), CipherError> {
    let params = params.split(',').map(str::trim).collect::<Vec<_>>();
    let Some(array_param) = params.first().copied().filter(|p| !p.is_empty()) else {
        return Err(CipherError::unsupported(format!(
            "helper method {method_name} has no array parameter"
        )));
    };
    let normalized = compact(body);
    let normalized = normalized.trim_end_matches(';');

    let reverse = format!("{array_param}.reverse()");
    if normalized == reverse {
        return Ok(());
    }

    if let Some(index_param) = params.get(1).copied().filter(|p| !p.is_empty()) {
        let splice = format!("{array_param}.splice(0,{index_param})");
        if normalized == splice {
            return Ok(());
        }

        if let Some(rest) = normalized.strip_prefix("var") {
            if let Some((temp, assignment)) = rest.split_once('=') {
                if is_identifier(temp) {
                    let index_plain = index_param.to_string();
                    let index_mod = format!("{index_param}%{array_param}.length");
                    let index_mod_paren = format!("({index_param}%{array_param}.length)");
                    for index_expr in [index_plain, index_mod, index_mod_paren] {
                        let expected = format!(
                            "{array_param}[0];{array_param}[0]={array_param}[{index_expr}];{array_param}[{index_expr}]={temp}"
                        );
                        if assignment == expected {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    Err(CipherError::unsupported(format!(
        "helper method {method_name} is not a classic reverse/splice/swap operation"
    )))
}

fn extract_method(object: &str, wanted: &str) -> Option<(String, String)> {
    let bytes = object.as_bytes();
    for (index, _) in object.match_indices(wanted) {
        if !identifier_boundaries(bytes, index, wanted.len()) {
            continue;
        }
        let mut cursor = skip_ws(bytes, index + wanted.len());
        if bytes.get(cursor) == Some(&b':') {
            cursor = skip_ws(bytes, cursor + 1);
            if object.get(cursor..)?.starts_with("function") {
                let header = parse_function_header(object, cursor)?;
                let close = find_matching(object, header.body_open, b'{', b'}')?;
                return Some((header.params, object.get(header.body_open + 1..close)?.to_string()));
            }
        } else if bytes.get(cursor) == Some(&b'(') {
            // Also accept object method shorthand: `r(a){a.reverse()}`.
            let params_end = find_matching(object, cursor, b'(', b')')?;
            let params = object.get(cursor + 1..params_end)?.to_string();
            cursor = skip_ws(bytes, params_end + 1);
            if bytes.get(cursor) == Some(&b'{') {
                let close = find_matching(object, cursor, b'{', b'}')?;
                return Some((params, object.get(cursor + 1..close)?.to_string()));
            }
        }
    }
    None
}

fn split_top_level_commas(source: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        Single,
        Double,
        Template,
    }

    let bytes = source.as_bytes();
    let mut state = State::Normal;
    let mut escaped = false;
    let mut round = 0usize;
    let mut curly = 0usize;
    let mut square = 0usize;
    let mut start = 0usize;
    let mut out = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        match state {
            State::Single | State::Double | State::Template => {
                let quote = match state {
                    State::Single => b'\'',
                    State::Double => b'"',
                    _ => b'`',
                };
                if escaped {
                    escaped = false;
                } else if *byte == b'\\' {
                    escaped = true;
                } else if *byte == quote {
                    state = State::Normal;
                }
            }
            State::Normal => match *byte {
                b'\'' => state = State::Single,
                b'"' => state = State::Double,
                b'`' => state = State::Template,
                b'(' => round += 1,
                b')' => round = round.saturating_sub(1),
                b'{' => curly += 1,
                b'}' => curly = curly.saturating_sub(1),
                b'[' => square += 1,
                b']' => square = square.saturating_sub(1),
                b',' if round == 0 && curly == 0 && square == 0 => {
                    out.push(source[start..index].trim().to_string());
                    start = index + 1;
                }
                _ => {}
            },
        }
    }
    out.push(source[start..].trim().to_string());
    out
}

/// The helper object itself must be data-only method definitions. Rejecting
/// value expressions here prevents an object literal such as `{x:ignored(),
/// r:function...}` from executing arbitrary player code inside QuickJS.
fn helper_object_has_only_methods(literal: &str) -> bool {
    let literal = literal.trim();
    if !literal.starts_with('{') || !literal.ends_with('}') {
        return false;
    }
    let inner = &literal[1..literal.len() - 1];
    let mut saw_method = false;
    for entry in split_top_level_commas(inner) {
        if entry.is_empty() {
            continue;
        }
        let bytes = entry.as_bytes();
        let Some((_name, mut cursor)) = parse_identifier(bytes, 0) else {
            return false;
        };
        cursor = skip_ws(bytes, cursor);
        let function_start = if bytes.get(cursor) == Some(&b':') {
            skip_ws(bytes, cursor + 1)
        } else {
            // Method shorthand: the parameter list starts immediately.
            cursor
        };
        let valid_function =
            entry.get(function_start..).is_some_and(|tail| tail.starts_with("function"))
                && parse_function_header(&entry, function_start)
                    .and_then(|header| find_matching(&entry, header.body_open, b'{', b'}'))
                    .is_some_and(|close| close + 1 == entry.len());
        let valid_shorthand = bytes.get(function_start) == Some(&b'(')
            && find_matching(&entry, function_start, b'(', b')')
                .and_then(|params_end| {
                    let body_open = skip_ws(bytes, params_end + 1);
                    (bytes.get(body_open) == Some(&b'{'))
                        .then(|| find_matching(&entry, body_open, b'{', b'}'))
                        .flatten()
                })
                .is_some_and(|close| close + 1 == entry.len());
        if !valid_function && !valid_shorthand {
            return false;
        }
        saw_method = true;
    }
    saw_method
}

fn extract_object_literal(js: &str, name: &str) -> Option<String> {
    let bytes = js.as_bytes();
    for (index, _) in js.match_indices(name) {
        if !identifier_boundaries(bytes, index, name.len()) {
            continue;
        }
        let mut cursor = skip_ws(bytes, index + name.len());
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor = skip_ws(bytes, cursor + 1);
        if bytes.get(cursor) != Some(&b'{') {
            continue;
        }
        let close = find_matching(js, cursor, b'{', b'}')?;
        return js.get(cursor..=close).map(str::to_string);
    }
    None
}

/// Statically restrict the transform to the documented classic shapes.
///
/// This scan is **defense-in-depth, not the security boundary**: it is a
/// text scan, so a sufficiently obfuscated transform could in principle sneak
/// constructs past it (concatenated identifiers, arrow functions, computed
/// member access). The actual containment boundary is the runtime itself —
/// the hardening preamble removes the dynamic-code machinery it would need
/// for escalation, and the memory/stack/wall-clock limits bound what any
/// smuggled code can do even if it executes. What this scan still buys is a
/// much smaller, auditable attack surface and an explicit unsupported-error
/// for modern players instead of wrong signatures.
fn validate_transform_source(
    context_js: &str,
    source: &str,
) -> Result<TransformProgram, CipherError> {
    if source.len() > MAX_TRANSFORM_SOURCE {
        return Err(CipherError::unsupported(
            "transform function is larger than the classic shape",
        ));
    }
    let source = source.trim();
    let header = parse_function_header(source, 0)
        .ok_or_else(|| CipherError::unsupported("transform is not a function expression"))?;
    let close = find_matching(source, header.body_open, b'{', b'}')
        .ok_or_else(|| CipherError::unsupported("transform function body is unterminated"))?;
    if close + 1 != source.len() {
        return Err(CipherError::unsupported("transform extraction has trailing JavaScript"));
    }
    let body = &source[header.body_open + 1..close];
    if body.len() > MAX_TRANSFORM_SOURCE {
        return Err(CipherError::unsupported("transform body is larger than the classic shape"));
    }
    let collapsed = compact(body);
    let splits = collapsed.contains(".split(\"\")") || collapsed.contains(".split('')");
    let joins = collapsed.contains(".join(\"\")") || collapsed.contains(".join('')");
    if !splits || !joins {
        return Err(CipherError::unsupported(
            "transform does not have the classic split-empty/join-empty shape",
        ));
    }
    for word in [
        "for",
        "while",
        "if",
        "switch",
        "try",
        "catch",
        "function",
        "eval",
        "Function",
        "import",
        "require",
        "fetch",
        "XMLHttpRequest",
    ] {
        if contains_word(body, word) {
            return Err(CipherError::unsupported(format!(
                "transform uses unsupported JavaScript feature `{word}`"
            )));
        }
    }

    let calls = member_calls(body);
    let mut object_methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (object, method) in calls {
        if matches!(method.as_str(), "split" | "join") {
            continue;
        }
        object_methods.entry(object).or_default().insert(method);
    }

    let mut helpers = Vec::new();
    for (object, methods) in object_methods {
        if let Some(literal) = extract_object_literal(context_js, &object) {
            if !helper_object_has_only_methods(&literal) {
                return Err(CipherError::unsupported(format!(
                    "helper object {object} contains non-method JavaScript"
                )));
            }
            if literal.len() > MAX_HELPER_SOURCE {
                return Err(CipherError::unsupported(format!(
                    "helper object {object} is larger than the classic shape"
                )));
            }
            for method in &methods {
                let Some((params, method_body)) = extract_method(&literal, method) else {
                    return Err(CipherError::unsupported(format!(
                        "helper object {object} does not define method {method} in classic form"
                    )));
                };
                validate_helper_method(&params, &method_body, method)?;
            }
            helpers.push((object, literal));
        } else if !methods.iter().all(|method| matches!(method.as_str(), "reverse" | "splice")) {
            return Err(CipherError::unsupported(format!(
                "transform references non-builtin member operations on {object}"
            )));
        }
    }

    Ok(TransformProgram { function_source: source.to_string(), helpers })
}

fn find_named_function(
    js: &str,
    headers: &[FunctionHeader],
    wanted: &str,
) -> Option<FunctionSource> {
    headers.iter().find_map(|header| {
        (assigned_name_before(js, header).as_deref() == Some(wanted))
            .then(|| function_source(js, header))
            .flatten()
    })
}

fn encode_uri_candidates(js: &str) -> Vec<String> {
    let bytes = js.as_bytes();
    let mut out = Vec::new();
    for (index, _) in js.match_indices("encodeURIComponent") {
        let mut cursor = skip_ws(bytes, index + "encodeURIComponent".len());
        if bytes.get(cursor) != Some(&b'(') {
            continue;
        }
        cursor = skip_ws(bytes, cursor + 1);
        if let Some((name, end)) = parse_identifier(bytes, cursor) {
            if bytes.get(skip_ws(bytes, end)) == Some(&b'(') {
                out.push(name);
            }
        }
    }
    out
}

fn sig_marker_candidates(js: &str) -> Vec<String> {
    let bytes = js.as_bytes();
    let mut out = Vec::new();
    for marker in [".sig||", "\"signature\"", "'signature'"] {
        for (index, _) in js.match_indices(marker) {
            let mut cursor = skip_ws(bytes, index + marker.len());
            if marker != ".sig||" {
                if bytes.get(cursor) != Some(&b',') {
                    continue;
                }
                cursor = skip_ws(bytes, cursor + 1);
            }
            if let Some((name, end)) = parse_identifier(bytes, cursor) {
                if bytes.get(skip_ws(bytes, end)) == Some(&b'(') {
                    out.push(name);
                }
            }
        }
    }
    out
}

/// Extract the classic signature transform. Call-site hints are tried first;
/// the final fallback is the distinctive small `split`/`join` transform shape.
pub(crate) fn extract_signature_transform(js: &str) -> Result<TransformProgram, CipherError> {
    let headers = collect_function_headers(js);
    let mut names = encode_uri_candidates(js);
    names.extend(sig_marker_candidates(js));
    names.dedup();
    for name in names {
        if let Some(function) = find_named_function(js, &headers, &name) {
            if let Ok(program) = validate_transform_source(js, &function.source) {
                return Ok(program);
            }
        }
    }

    for header in &headers {
        // Classic transforms split their input at the start of a small body.
        // Probe before brace matching so unsupported modern `base.js` files do
        // not trigger repeated scans of huge nested functions.
        let probe_start = header.body_open.saturating_add(1);
        let probe_end = probe_start.saturating_add(512).min(js.len());
        if !js.get(probe_start..probe_end).is_some_and(|probe| probe.contains(".split")) {
            continue;
        }
        let Some(function) = function_source(js, header) else {
            continue;
        };
        if function.source.len() <= MAX_TRANSFORM_SOURCE {
            if let Ok(program) = validate_transform_source(js, &function.source) {
                return Ok(program);
            }
        }
    }
    Err(CipherError::unsupported("no classic reverse/splice/swap signature function was found"))
}

fn expect_byte(bytes: &[u8], cursor: &mut usize, expected: u8) -> bool {
    *cursor = skip_ws(bytes, *cursor);
    if bytes.get(*cursor) == Some(&expected) {
        *cursor += 1;
        true
    } else {
        false
    }
}

fn n_call_site(js: &str) -> Option<(String, Option<usize>)> {
    let bytes = js.as_bytes();
    for marker in [".get(\"n\")", ".get('n')"] {
        for (index, _) in js.match_indices(marker) {
            let mut cursor = index + marker.len();
            if !expect_byte(bytes, &mut cursor, b')')
                || !expect_byte(bytes, &mut cursor, b'&')
                || !expect_byte(bytes, &mut cursor, b'&')
                || !expect_byte(bytes, &mut cursor, b'(')
            {
                continue;
            }
            // Historical players assign the result to the minified variable `b`.
            let Some((target, after_target)) = parse_identifier(bytes, cursor) else {
                continue;
            };
            if target != "b" {
                continue;
            }
            cursor = after_target;
            if !expect_byte(bytes, &mut cursor, b'=') {
                continue;
            }
            cursor = skip_ws(bytes, cursor);
            let Some((callee, after_callee)) = parse_identifier(bytes, cursor) else {
                continue;
            };
            cursor = skip_ws(bytes, after_callee);
            let mut array_index = None;
            if bytes.get(cursor) == Some(&b'[') {
                cursor += 1;
                let digits_start = cursor;
                while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                    cursor += 1;
                }
                if digits_start == cursor || bytes.get(cursor) != Some(&b']') {
                    continue;
                }
                array_index = std::str::from_utf8(&bytes[digits_start..cursor]).ok()?.parse().ok();
                cursor += 1;
            }
            cursor = skip_ws(bytes, cursor);
            if bytes.get(cursor) == Some(&b'(') {
                return Some((callee, array_index));
            }
        }
    }
    None
}

fn extract_array_literal(js: &str, name: &str) -> Option<String> {
    let bytes = js.as_bytes();
    for (index, _) in js.match_indices(name) {
        if !identifier_boundaries(bytes, index, name.len()) {
            continue;
        }
        let mut cursor = skip_ws(bytes, index + name.len());
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor = skip_ws(bytes, cursor + 1);
        if bytes.get(cursor) != Some(&b'[') {
            continue;
        }
        let close = find_matching(js, cursor, b'[', b']')?;
        return js.get(cursor + 1..close).map(str::to_string);
    }
    None
}

enum Callee {
    Named(String),
    Inline(String),
}

fn resolve_indexed_callee(js: &str, array_name: &str, index: usize) -> Result<Callee, CipherError> {
    let Some(array) = extract_array_literal(js, array_name) else {
        return Err(CipherError::unsupported(format!(
            "n-transform array {array_name} was not found in classic form"
        )));
    };
    let entries = split_top_level_commas(&array);
    let Some(entry) = entries.get(index).map(String::as_str).filter(|entry| !entry.is_empty())
    else {
        return Err(CipherError::unsupported(format!(
            "n-transform array {array_name} has no entry {index}"
        )));
    };
    if entry.starts_with("function") {
        return Ok(Callee::Inline(entry.to_string()));
    }
    if is_identifier(entry) {
        return Ok(Callee::Named(entry.to_string()));
    }
    if (entry.starts_with('"') && entry.ends_with('"'))
        || (entry.starts_with('\'') && entry.ends_with('\''))
    {
        if let Ok(name) = serde_json::from_str::<String>(&entry.replace('\'', "\"")) {
            if is_identifier(&name) {
                return Ok(Callee::Named(name));
            }
        }
    }
    Err(CipherError::unsupported("indexed n-transform entry is not a function or identifier"))
}

/// Extract the classic `n` transform from its `.get("n")` call site. Modern
/// opcode-array n functions intentionally fail here; they need an AST-grade
/// solver, not this compatibility layer.
pub(crate) fn extract_n_transform(js: &str) -> Result<TransformProgram, CipherError> {
    let (callee, index) = n_call_site(js).ok_or_else(|| {
        CipherError::unsupported("no classic .get(\"n\") transform call site was found")
    })?;
    let headers = collect_function_headers(js);
    let source = if let Some(array_index) = index {
        match resolve_indexed_callee(js, &callee, array_index)? {
            Callee::Named(name) => {
                find_named_function(js, &headers, &name)
                    .ok_or_else(|| {
                        CipherError::unsupported(format!(
                            "n-transform function {name} was not found"
                        ))
                    })?
                    .source
            }
            Callee::Inline(source) => source,
        }
    } else {
        find_named_function(js, &headers, &callee)
            .ok_or_else(|| {
                CipherError::unsupported(format!("n-transform function {callee} was not found"))
            })?
            .source
    };
    validate_transform_source(js, &source)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSIC_PLAYER: &str = r#"
var Xh={
 r:function(a){a.reverse()},
 s:function(a,b){a.splice(0,b)},
 w:function(a,b){var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}
};
function xsig(a){a=a.split("");Xh.w(a,5);Xh.s(a,1);Xh.r(a);Xh.w(a,2);return a.join("")}
var decoder=function(x){return x.sig||xsig(x)};
function xn(a){a=a.split("");Xh.r(a);Xh.w(a,4);return a.join("")}
if (u.get("n"))&&(b=xn(b)){};
"#;

    #[test]
    fn classic_signature_transform_executes_in_quickjs() {
        let program = extract_signature_transform(CLASSIC_PLAYER).unwrap();
        assert_eq!(program.apply("abcdefghijklmnopqrst").unwrap(), "rstqponmlkjihgaedcb");
    }

    #[test]
    fn classic_n_transform_executes_in_quickjs() {
        let program = extract_n_transform(CLASSIC_PLAYER).unwrap();
        assert_eq!(program.apply("abcdefg").unwrap(), "cfedgba");
    }

    #[test]
    fn reverse_splice_and_swap_shapes_execute_individually() {
        fn signature_js(operation: &str) -> String {
            format!(
                r#"var AB={{
 r(a){{a.reverse()}},
 s(a,b){{a.splice(0,b)}},
 w(a,b){{var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}}
}};
function sig(a){{a=a.split("");{operation}return a.join("")}}
var decoder=function(x){{return x.sig||sig(x)}};"#
            )
        }

        let reverse = extract_signature_transform(&signature_js("AB.r(a);")).unwrap();
        assert_eq!(reverse.apply("abcdef").unwrap(), "fedcba");
        let splice = extract_signature_transform(&signature_js("AB.s(a,2);")).unwrap();
        assert_eq!(splice.apply("abcdef").unwrap(), "cdef");
        let swap = extract_signature_transform(&signature_js("AB.w(a,2);")).unwrap();
        assert_eq!(swap.apply("abcdef").unwrap(), "cbadef");
    }

    #[test]
    fn direct_array_method_shape_executes() {
        let js = r#"
function sig(a){a=a.split("");a.reverse();return a.join("")}
var decoder=function(x){return x.sig||sig(x)};
"#;
        let program = extract_signature_transform(js).unwrap();
        assert_eq!(program.apply("abcdef").unwrap(), "fedcba");
    }

    #[test]
    fn indexed_and_inline_n_call_sites_execute() {
        let indexed = r#"
function xn(a){a=a.split("");a.reverse();return a.join("")}
var NA=[xn];
if (u.get("n"))&&(b=NA[0](b)){};
"#;
        let program = extract_n_transform(indexed).unwrap();
        assert_eq!(program.apply("abcdefg").unwrap(), "gfedcba");

        let inline = r#"
var NI=[function(a){a=a.split("");a.reverse();return a.join("")}];
if (u.get("n"))&&(b=NI[0](b)){};
"#;
        let program = extract_n_transform(inline).unwrap();
        assert_eq!(program.apply("abcdefg").unwrap(), "gfedcba");
    }

    #[test]
    fn opaque_helpers_and_dynamic_evaluation_are_rejected() {
        let opaque_helper = r#"
var AB={x(a){a.reverse();a.splice(0,1)}};
function sig(a){a=a.split("");AB.x(a);return a.join("")}
var decoder=function(x){return x.sig||sig(x)};
"#;
        assert!(matches!(
            extract_signature_transform(opaque_helper),
            Err(CipherError::Unsupported(_))
        ));

        let dynamic_eval = r#"
function sig(a){a=a.split("");eval("a.reverse()");return a.join("")}
var decoder=function(x){return x.sig||sig(x)};
"#;
        assert!(matches!(
            extract_signature_transform(dynamic_eval),
            Err(CipherError::Unsupported(_))
        ));
    }

    #[test]
    fn control_flow_transforms_are_rejected() {
        let js = r#"
function bad(a){a=a.split("");for(var i=0;i<a.length;i++){a[i]=a[i]}return a.join("")}
var decoder=function(x){return x.sig||bad(x)};
"#;
        assert!(matches!(extract_signature_transform(js), Err(CipherError::Unsupported(_))));
    }

    // --- Sandbox adversarial tests --------------------------------------
    //
    // These tests bypass the static shape validation on purpose by building
    // `TransformProgram` directly: they model an attacker who found a way past
    // the text scan and probe the runtime containment boundary itself.

    fn raw_program(function_source: &str) -> TransformProgram {
        TransformProgram { function_source: function_source.to_string(), helpers: Vec::new() }
    }

    /// Evaluate `probe` inside a runtime hardened exactly the way `apply`
    /// hardens it, returning the probe's string result or the failure text.
    fn eval_in_hardened_sandbox(probe: &str) -> Result<String, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(JS_MEMORY_LIMIT);
        runtime.set_max_stack_size(JS_STACK_LIMIT);
        let context = Context::builder()
            .with::<intrinsic::Eval>()
            .with::<intrinsic::Promise>()
            .build(&runtime)
            .map_err(|e| e.to_string())?;
        let mut script = HARDENING_PREAMBLE.to_string();
        script.push_str("\nString((function(){ try { ");
        script.push_str(probe);
        script.push_str(" } catch (e) { return 'threw'; } })())");
        context.with(|ctx| ctx.eval::<String, _>(script)).map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn wall_clock_deadline_terminates_infinite_loop() {
        // Isolate the wall-clock path: the count budget can never fire here.
        let program = raw_program("function x(a){while(1){}return a}");
        let start = Instant::now();
        let result = program.apply_with_limits("abc", Duration::from_millis(150), usize::MAX);
        let elapsed = start.elapsed();
        assert!(matches!(result, Err(CipherError::Failed(_))), "got {result:?}");
        assert!(
            elapsed < Duration::from_secs(3),
            "infinite loop was not interrupted promptly ({elapsed:?})"
        );
    }

    #[test]
    fn interrupt_count_budget_terminates_infinite_loop() {
        // The count budget alone (no wall clock) still terminates tight loops.
        let program = raw_program("function x(a){while(1){}return a}");
        let start = Instant::now();
        let result = program.apply_with_limits("abc", Duration::from_secs(60), 20_000);
        let elapsed = start.elapsed();
        assert!(matches!(result, Err(CipherError::Failed(_))), "got {result:?}");
        assert!(
            elapsed < Duration::from_secs(10),
            "count budget did not interrupt the loop ({elapsed:?})"
        );
    }

    #[test]
    fn wall_clock_covers_heavy_loop_bodies() {
        // A loop with a non-trivial body: few backward-branch events per unit
        // of wall time, exactly the case the count budget cannot bound.
        let program = raw_program(
            "function x(a){var s=[];while(1){s.push(1);if(s.length>64){s=[]}}return a}",
        );
        let start = Instant::now();
        let result = program.apply_with_limits("abc", Duration::from_millis(150), usize::MAX);
        let elapsed = start.elapsed();
        assert!(matches!(result, Err(CipherError::Failed(_))), "got {result:?}");
        assert!(elapsed < Duration::from_secs(3), "heavy loop not interrupted ({elapsed:?})");
    }

    #[test]
    fn worker_survives_hostile_job_and_processes_next() {
        let hostile = raw_program("function x(a){while(1){}return a}");
        assert!(hostile.apply_with_limits("abc", Duration::from_millis(100), usize::MAX).is_err());
        let legit = extract_signature_transform(CLASSIC_PLAYER).unwrap();
        assert_eq!(legit.apply("abcdefghijklmnopqrst").unwrap(), "rstqponmlkjihgaedcb");
    }

    #[test]
    fn constructor_chain_codegen_is_severed() {
        // `obj.constructor.constructor` is the Function constructor in an
        // unhardened runtime; here it must fail fast instead of compiling.
        for (label, source) in [
            (
                "object-chain",
                "function x(a){({}).constructor.constructor('while(1){}')();return a}",
            ),
            (
                "generator-chain",
                "function x(a){(function*(){}).constructor('while(1){}')();return a}",
            ),
            (
                "async-chain",
                "function x(a){(async function(){}).constructor('while(1){}')();return a}",
            ),
            (
                "async-generator-chain",
                "function x(a){(async function*(){}).constructor('while(1){}')();return a}",
            ),
            ("new-Function", "function x(a){new Function('while(1){}')();return a}"),
            ("concat-eval", "function x(a){this['ev'+'al']('whi'+'le(1){}');return a}"),
        ] {
            let program = raw_program(source);
            let start = Instant::now();
            let result = program.apply_with_limits("abc", Duration::from_secs(5), usize::MAX);
            assert!(
                matches!(result, Err(CipherError::Failed(_))),
                "{label}: codegen escape executed instead of failing ({result:?})"
            );
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "{label}: escape attempt hung instead of failing fast"
            );
        }
    }

    #[test]
    fn prototype_pollution_is_frozen_out() {
        // Scripts are evaluated in strict mode (the rquickjs default), so the
        // write itself already throws; even an attacker who catches that and
        // continues finds the genuine intrinsics unmodified.
        let loud = raw_program(
            "function x(a){Array.prototype.reverse=function(){return['z']};a=a.split('');a.reverse();return a.join('')}",
        );
        assert!(matches!(loud.apply("abcdef"), Err(CipherError::Failed(_))));

        let adaptive = raw_program(
            "function x(a){try{Array.prototype.reverse=function(){return['z']}}catch(e){}try{Object.prototype.evil=1}catch(e){}a=a.split('');a.reverse();return typeof Object.prototype.evil+':'+a.join('')}",
        );
        assert_eq!(adaptive.apply("abcdef").unwrap(), "undefined:fedcba");
    }

    #[test]
    fn fresh_runtime_per_job_isolates_pollution() {
        // Even if pollution somehow succeeded inside one job, the next job
        // runs in a fresh runtime and must be unaffected.
        let pollute = raw_program(
            "function x(a){try{Object.prototype.evil=42}catch(e){}try{Array.prototype.join=function(){return'x'}}catch(e){}return a}",
        );
        let _ = pollute.apply("abcdef");
        let check = raw_program(
            "function x(a){a=a.split('');a.reverse();return typeof Object.prototype.evil+':'+a.join('')}",
        );
        assert_eq!(check.apply("abcdef").unwrap(), "undefined:fedcba");
    }

    #[test]
    fn memory_and_stack_bombs_are_bounded() {
        let memory = raw_program("function x(a){var s='x';while(1){s+=s}return a}");
        let start = Instant::now();
        assert!(memory.apply("abc").is_err());
        assert!(start.elapsed() < Duration::from_secs(5), "memory bomb outlived the limits");

        let recursion = raw_program("function x(a){var f=function(){return f()};f();return a}");
        let start = Instant::now();
        assert!(recursion.apply("abc").is_err());
        assert!(start.elapsed() < Duration::from_secs(5), "recursion bomb outlived the limits");
    }

    #[test]
    fn async_function_objects_do_not_crash_teardown() {
        // Async functions cannot even be *created* without crashing the
        // process at runtime free time unless the context is set up for them;
        // this test passing at all is the evidence (it aborts otherwise).
        for source in [
            "function x(a){var f=async()=>1;return a}",
            "function x(a){var f=async function(){return 1};return a}",
            "function x(a){var g=async function*(){yield 1};return a}",
            "function x(a){var f=async()=>1;var g=async()=>2;var h=async()=>3;return a}",
        ] {
            let program = raw_program(source);
            assert_eq!(program.apply("abc").unwrap(), "abc");
        }
    }

    #[test]
    fn microtasks_never_execute() {
        let program = raw_program(
            "function x(a){var h=0;queueMicrotask(function(){h=1});Promise.resolve().then(function(){h=2});return a+h}",
        );
        assert_eq!(program.apply("abc").unwrap(), "abc0");
    }

    #[test]
    fn deep_scan_finds_no_surviving_codegen() {
        // Walk every object reachable inside the hardened runtime and try
        // calling every function-valued property as a code compiler. Any
        // surviving eval/Function-style capability shows up here.
        let result = eval_in_hardened_sandbox(
            r#"
      var seen=[]; var queue=[globalThis, function*(){}, async function(){}, async function*(){}];
      function enq(v){ if((typeof v==='object'&&v!==null)||typeof v==='function'){ if(seen.indexOf(v)===-1){ seen.push(v); queue.push(v) } } }
      var cursor=0;
      while(cursor<queue.length){
        var obj=queue[cursor++];
        var names=[];
        try{ names=Object.getOwnPropertyNames(obj) }catch(e){}
        for(var j=0;j<names.length;j++){
          var d; try{ d=Object.getOwnPropertyDescriptor(obj,names[j]) }catch(e){ continue }
          if(!d) continue;
          if('value' in d) enq(d.value);
          if(d.get) enq(d.get);
          if(d.set) enq(d.set);
        }
        try{ var p=Object.getPrototypeOf(obj); if(p) enq(p) }catch(e){}
      }
      var pwned=[];
      for(var k=0;k<seen.length;k++){
        var f=seen[k];
        if(typeof f!=='function') continue;
        try {
          var out=f('return 1337');
          if(typeof out==='function'){ pwned.push('compiles:'+(f.name||'anon')) }
        } catch(e){}
      }
      return 'scanned:'+seen.length+' pwned:'+(pwned.length?pwned.join(','):'none');
            "#,
        );
        let report = result.expect("deep scan probe itself must run");
        assert!(
            report.contains("pwned:none"),
            "surviving codegen capability found in hardened sandbox: {report}"
        );
        // Sanity: the scan must actually have covered the object graph — a
        // walk that silently broke after a handful of objects would make the
        // pwned:none result meaningless. The hardened base context exposes
        // roughly 300 reachable objects.
        let scanned: usize = report
            .strip_prefix("scanned:")
            .and_then(|rest| rest.split(' ').next())
            .and_then(|count| count.parse().ok())
            .expect("scan report carries the object count");
        assert!(scanned > 100, "object graph walk suspiciously shallow: {report}");
    }

    #[test]
    fn intrinsics_are_frozen_and_globals_dropped() {
        let result = eval_in_hardened_sandbox(
            r#"
      return 'eval:'+typeof globalThis.eval
        +' Function:'+typeof globalThis.Function
        +' objproto:'+Object.isFrozen(Object.prototype)
        +' fnproto:'+Object.isFrozen(Object.getPrototypeOf(function(){}))
        +' arrproto:'+Object.isFrozen(Array.prototype)
        +' strproto:'+Object.isFrozen(String.prototype)
        +' global:'+Object.isFrozen(globalThis)
        +' ctorhop:'+typeof ({}).constructor
        +' genhop:'+typeof (function*(){}).constructor
            "#,
        )
        .expect("freeze probe must run");
        assert_eq!(
            result,
            "eval:undefined Function:undefined objproto:true fnproto:true arrproto:true strproto:true global:true ctorhop:undefined genhop:undefined"
        );
    }

    #[test]
    fn legitimate_cipher_latency_stays_small() {
        // The hardening preamble runs per job; keep the total cost in the
        // millisecond range so extraction of many formats stays cheap.
        // Locally this measures ~1ms per application; the bound is generous
        // for slow or heavily parallel CI while still catching accidental
        // orders-of-magnitude regressions.
        let program = extract_signature_transform(CLASSIC_PLAYER).unwrap();
        let start = Instant::now();
        for _ in 0..10 {
            assert_eq!(program.apply("abcdefghijklmnopqrst").unwrap(), "rstqponmlkjihgaedcb");
        }
        let each = start.elapsed() / 10;
        eprintln!("cipher apply latency: {each:?} per application");
        assert!(each < Duration::from_millis(150), "apply latency regressed: {each:?}");
    }
}
