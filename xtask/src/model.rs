//! Contract schemas -> Rust types (serde structs and enums).

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "else", "enum", "extern", "false", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static",
    "struct", "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn",
    "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized",
    "virtual", "yield", "try", "gen",
];

/// PascalCase from any separator-delimited name (`get_wallet` -> `GetWallet`).
pub fn pascal(s: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            up = true;
            continue;
        }
        if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, 'N');
    }
    out
}

/// snake_case from camelCase / kebab-case names (`XBuyerToken` -> `x_buyer_token`).
pub fn snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            continue;
        }
        if c.is_uppercase() {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                Some(p) if p.is_lowercase() || p.is_ascii_digit() => true,
                Some(p) if p.is_uppercase() => next.is_some_and(|n| n.is_lowercase()),
                _ => false,
            };
            if boundary && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, 'f');
    }
    out
}

/// A field or method identifier, raw-escaped when it is a Rust keyword.
pub fn ident(s: &str) -> String {
    if matches!(s, "self" | "Self" | "super" | "crate") {
        return format!("{s}_");
    }
    if KEYWORDS.contains(&s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}

/// Collapses a description to one line that cannot open a doc code block.
pub fn doc_line(s: &str) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let one = one.replace("```", "`").replace("~~~", "~");
    one.trim().to_string()
}

pub fn doc_comment(s: Option<&str>, indent: &str) -> String {
    match s.map(doc_line) {
        Some(d) if !d.is_empty() => format!("{indent}/// {d}\n"),
        _ => String::new(),
    }
}

pub struct Types<'a> {
    doc: &'a Value,
    /// Rendered definitions by Rust type name (sorted for deterministic output).
    defs: BTreeMap<String, String>,
    /// Names that are taken (components first, then inline types).
    reserved: BTreeSet<String>,
}

/// A resolved Rust type; `nullable` means the schema allows `null`.
pub struct Ty {
    pub rust: String,
    pub nullable: bool,
}

pub fn schema_name(component: &str) -> String {
    let n = pascal(component);
    // `Error` is the contract's error envelope; the SDK's own error type owns that name.
    if n == "Error" {
        "ErrorResponse".to_string()
    } else {
        n
    }
}

impl<'a> Types<'a> {
    pub fn new(doc: &'a Value) -> Self {
        let mut reserved = BTreeSet::new();
        if let Some(s) = doc
            .pointer("/components/schemas")
            .and_then(Value::as_object)
        {
            for k in s.keys() {
                reserved.insert(schema_name(k));
            }
        }
        Types {
            doc,
            defs: BTreeMap::new(),
            reserved,
        }
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn define_components(&mut self) -> Result<(), String> {
        let schemas = self
            .doc
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .ok_or("contract without components.schemas")?;
        for (name, schema) in schemas {
            self.define(&schema_name(name), schema)?;
        }
        Ok(())
    }

    /// Reserves a fresh name for an inline type (suffixing on collisions).
    pub fn claim(&mut self, want: &str) -> String {
        let mut name = want.to_string();
        let mut n = 2;
        while self.reserved.contains(&name) {
            name = format!("{want}{n}");
            n += 1;
        }
        self.reserved.insert(name.clone());
        name
    }

    /// Defines `name` (already reserved) for a top-level schema, as a struct,
    /// an enum, or a type alias.
    pub fn define(&mut self, name: &str, schema: &Value) -> Result<(), String> {
        if self.defs.contains_key(name) {
            return Ok(());
        }
        // Placeholder so self-references resolve.
        self.defs.insert(name.to_string(), String::new());
        let ty = self.build(schema, name, Some(name))?;
        if ty.rust != name {
            let doc = doc_comment(schema.get("description").and_then(Value::as_str), "");
            let target = ty.rust;
            self.defs.insert(
                name.to_string(),
                format!("{doc}pub type {name} = {target};\n"),
            );
        }
        Ok(())
    }

    /// Defines an operation-level inline type under a freshly claimed name.
    pub fn define_inline(&mut self, want: &str, schema: &Value) -> Result<String, String> {
        let name = self.claim(want);
        self.define(&name, schema)?;
        Ok(name)
    }

    fn deref(&self, r: &str) -> Result<&'a Value, String> {
        let name = r
            .strip_prefix("#/components/schemas/")
            .ok_or_else(|| format!("unsupported $ref: {r}"))?;
        self.doc
            .pointer("/components/schemas")
            .and_then(|s| s.get(name))
            .ok_or_else(|| format!("$ref without target: {r}"))
    }

    /// Collects properties and required names, following `$ref` and `allOf`.
    fn collect(
        &self,
        schema: &Value,
        props: &mut BTreeMap<String, Value>,
        required: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 16 {
            return Err("allOf nesting too deep".into());
        }
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            return self.collect(self.deref(r)?, props, required, depth + 1);
        }
        if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
            for m in all {
                self.collect(m, props, required, depth + 1)?;
            }
        }
        if let Some(p) = schema.get("properties").and_then(Value::as_object) {
            for (k, v) in p {
                props.insert(k.clone(), v.clone());
            }
        }
        if let Some(r) = schema.get("required").and_then(Value::as_array) {
            for k in r.iter().filter_map(Value::as_str) {
                required.insert(k.to_string());
            }
        }
        Ok(())
    }

    fn type_names(schema: &Value) -> (Vec<String>, bool) {
        let mut names = vec![];
        let mut nullable = schema.get("nullable").and_then(Value::as_bool) == Some(true);
        match schema.get("type") {
            Some(Value::String(s)) if s == "null" => nullable = true,
            Some(Value::String(s)) => names.push(s.clone()),
            Some(Value::Array(a)) => {
                for t in a.iter().filter_map(Value::as_str) {
                    if t == "null" {
                        nullable = true;
                    } else {
                        names.push(t.to_string());
                    }
                }
            }
            _ => {}
        }
        (names, nullable)
    }

    /// Resolves a schema to a Rust type. With `top = Some(name)` structs and
    /// enums are defined under exactly that name; otherwise inline ones get a
    /// name derived from `hint`.
    pub fn build(&mut self, schema: &Value, hint: &str, top: Option<&str>) -> Result<Ty, String> {
        let plain = |rust: &str, nullable: bool| Ty {
            rust: rust.to_string(),
            nullable,
        };
        let Some(obj) = schema.as_object() else {
            return Ok(plain("serde_json::Value", false)); // `true` / empty schema
        };

        if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
            let name = r
                .strip_prefix("#/components/schemas/")
                .ok_or_else(|| format!("unsupported $ref: {r}"))?;
            return Ok(plain(&schema_name(name), false));
        }

        let (types, mut nullable) = Self::type_names(schema);

        // allOf: one bare $ref is an alias, anything else is a merged struct.
        if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
            let only_refs =
                all.len() == 1 && all[0].get("$ref").is_some() && !obj.contains_key("properties");
            if only_refs {
                let mut t = self.build(&all[0], hint, None)?;
                t.nullable |= nullable;
                return Ok(t);
            }
            let name = self.type_name(hint, top);
            self.emit_struct(&name, schema)?;
            return Ok(plain(&name, nullable));
        }

        for key in ["oneOf", "anyOf"] {
            let Some(members) = obj.get(key).and_then(Value::as_array) else {
                continue;
            };
            let mut rest = vec![];
            for m in members {
                if Self::type_names(m) == (vec![], true)
                    && m.as_object().is_some_and(|o| o.len() == 1)
                {
                    nullable = true;
                } else {
                    rest.push(m);
                }
            }
            if obj.contains_key("properties") {
                break; // constraints on the properties below; ignore the oneOf
            }
            if rest.len() == 1 {
                let mut t = self.build(rest[0], hint, top)?;
                t.nullable |= nullable;
                return Ok(t);
            }
            if !rest.is_empty() && rest.iter().all(|m| m.get("$ref").is_some()) {
                let name = self.type_name(hint, top);
                self.emit_untagged(&name, schema, &rest)?;
                return Ok(plain(&name, nullable));
            }
            return Ok(plain("serde_json::Value", nullable));
        }

        if let Some(values) = obj.get("enum").and_then(Value::as_array) {
            let strings: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
            if !strings.is_empty()
                && strings.len() == values.iter().filter(|v| !v.is_null()).count()
            {
                let name = self.type_name(hint, top);
                self.emit_enum(&name, schema, &strings);
                return Ok(plain(&name, nullable || values.iter().any(Value::is_null)));
            }
        }

        // `const` carries its own type when the schema declares none.
        let const_kind = match obj.get("const") {
            Some(Value::String(_)) => Some("string"),
            Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Some("integer"),
            Some(Value::Number(_)) => Some("number"),
            Some(Value::Bool(_)) => Some("boolean"),
            _ => None,
        };
        let kind = types.first().map(String::as_str).or(const_kind);
        match kind {
            Some("string") => Ok(plain("String", nullable)),
            Some("integer") => Ok(plain("i64", nullable)),
            Some("number") => Ok(plain("f64", nullable)),
            Some("boolean") => Ok(plain("bool", nullable)),
            Some("array") => {
                let items = obj.get("items").cloned().unwrap_or(Value::Bool(true));
                let inner = self.build(&items, &format!("{hint}Item"), None)?;
                let elem = if inner.nullable {
                    format!("Option<{}>", inner.rust)
                } else {
                    inner.rust
                };
                Ok(plain(&format!("Vec<{elem}>"), nullable))
            }
            Some("object") | None if obj.contains_key("properties") || kind == Some("object") => {
                if obj.contains_key("properties") {
                    let name = self.type_name(hint, top);
                    self.emit_struct(&name, schema)?;
                    return Ok(plain(&name, nullable));
                }
                match obj.get("additionalProperties") {
                    Some(ap) if ap.is_object() => {
                        let inner = self.build(ap, &format!("{hint}Value"), None)?;
                        let elem = if inner.nullable {
                            format!("Option<{}>", inner.rust)
                        } else {
                            inner.rust
                        };
                        Ok(plain(
                            &format!("std::collections::HashMap<String, {elem}>"),
                            nullable,
                        ))
                    }
                    _ => Ok(plain("serde_json::Value", nullable)),
                }
            }
            _ => Ok(plain("serde_json::Value", nullable)),
        }
    }

    fn type_name(&mut self, hint: &str, top: Option<&str>) -> String {
        match top {
            Some(t) => t.to_string(),
            None => self.claim(hint),
        }
    }

    fn emit_struct(&mut self, name: &str, schema: &Value) -> Result<(), String> {
        self.defs.entry(name.to_string()).or_default();
        let mut props = BTreeMap::new();
        let mut required = BTreeSet::new();
        self.collect(schema, &mut props, &mut required, 0)?;

        let mut out = doc_comment(schema.get("description").and_then(Value::as_str), "");
        out.push_str("#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]\n");
        out.push_str(&format!("pub struct {name} {{\n"));
        let mut used = BTreeSet::new();
        for (key, prop) in &props {
            let mut field = snake(key);
            while used.contains(&field) {
                field.push('_');
            }
            used.insert(field.clone());
            let ty = self.build(prop, &format!("{name}{}", pascal(key)), None)?;
            let is_required = required.contains(key);
            let mut rust = ty.rust.clone();
            if rust == name {
                rust = format!("Box<{rust}>");
            }
            let optional = !is_required || ty.nullable;
            let mut attrs = vec![];
            if field != *key {
                attrs.push(format!("rename = \"{key}\""));
            }
            if !is_required {
                attrs.push("default".to_string());
                attrs.push("skip_serializing_if = \"Option::is_none\"".to_string());
            } else if ty.nullable {
                attrs.push("default".to_string());
            }
            out.push_str(&doc_comment(
                prop.get("description").and_then(Value::as_str),
                "    ",
            ));
            if !attrs.is_empty() {
                out.push_str(&format!("    #[serde({})]\n", attrs.join(", ")));
            }
            let full = if optional {
                format!("Option<{rust}>")
            } else {
                rust
            };
            out.push_str(&format!("    pub {}: {full},\n", ident(&field)));
        }
        out.push_str("}\n");
        self.defs.insert(name.to_string(), out);
        Ok(())
    }

    fn emit_untagged(
        &mut self,
        name: &str,
        schema: &Value,
        members: &[&Value],
    ) -> Result<(), String> {
        let mut variants: Vec<String> = vec![];
        for m in members {
            let r = m.get("$ref").and_then(Value::as_str).unwrap_or_default();
            let target = self.deref(r)?;
            let _ = target;
            variants.push(schema_name(r.rsplit('/').next().unwrap_or_default()));
        }
        let mut out = doc_comment(schema.get("description").and_then(Value::as_str), "");
        out.push_str(
            "#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n#[serde(untagged)]\n",
        );
        out.push_str(&format!("pub enum {name} {{\n"));
        for v in &variants {
            out.push_str(&format!("    {v}({v}),\n"));
        }
        out.push_str("}\n\n");
        let first = &variants[0];
        out.push_str(&format!(
            "impl Default for {name} {{\n    fn default() -> Self {{\n        {name}::{first}(Default::default())\n    }}\n}}\n"
        ));
        self.defs.insert(name.to_string(), out);
        Ok(())
    }

    fn emit_enum(&mut self, name: &str, schema: &Value, values: &[&str]) {
        let mut out = doc_comment(schema.get("description").and_then(Value::as_str), "");
        out.push_str(
            "#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]\n",
        );
        out.push_str(&format!("pub enum {name} {{\n"));
        let mut used: BTreeSet<String> = BTreeSet::new();
        used.insert("Unknown".to_string());
        for (i, v) in values.iter().enumerate() {
            let mut variant = pascal(v);
            if variant.is_empty() {
                variant = "Empty".to_string();
            }
            let base = variant.clone();
            let mut n = 2;
            while used.contains(&variant) {
                variant = format!("{base}{n}");
                n += 1;
            }
            used.insert(variant.clone());
            if i == 0 {
                out.push_str("    #[default]\n");
            }
            out.push_str(&format!("    #[serde(rename = \"{v}\")]\n    {variant},\n"));
        }
        out.push_str(
            "    /// A value this version of the SDK does not know yet.\n    #[serde(other)]\n    Unknown,\n}\n",
        );
        self.defs.insert(name.to_string(), out);
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "// Code generated by `cargo xtask generate` from api/openapi.sdk.json. DO NOT EDIT.\n\
             \n\
             //! Types of the public API contract.\n\
             \n\
             #![allow(clippy::all, unused_imports, missing_docs)]\n\
             \n\
             use serde::{Deserialize, Serialize};\n\n",
        );
        for def in self.defs.values() {
            out.push_str(def);
            out.push('\n');
        }
        out
    }
}
