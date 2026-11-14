//! Contract operations -> `client.<tag>().<action>(...)` facade.

use crate::model::{doc_line, ident, pascal, snake, Types};
use serde_json::Value;
use std::collections::BTreeSet;

type Obj = serde_json::Map<String, Value>;

/// Fixed HTTP method order inside a path (same as the Go and TypeScript generators).
const HTTP_METHODS: &[&str] = &["get", "put", "post", "delete", "patch", "options", "head"];

/// Headers the transport owns (call options); they never appear in `Headers`.
const TRANSPORT_HEADERS: &[&str] = &["idempotency-key", "if-match", "x-infi-channel"];

pub struct Op {
    pub operation_id: String,
    pub tag: String,
    pub action: String,
    pub method: String,
    pub path: String,
    pub scope: String,
    pub summary: String,
    raw: Obj,
    item: Obj,
}

fn is_name(s: &str) -> bool {
    // ^[a-z][a-z0-9]*(_[a-z0-9]+)*$
    let mut chars = s.chars().peekable();
    if !chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let mut prev_us = false;
    for c in s.chars().skip(1) {
        if c == '_' {
            if prev_us {
                return false;
            }
            prev_us = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            prev_us = false;
        } else {
            return false;
        }
    }
    !prev_us
}

/// Validates the contract and returns its operations, sorted by path.
pub fn collect_ops(doc: &Value) -> Result<Vec<Op>, String> {
    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or("contract without paths")?;
    let mut ops = vec![];
    let mut seen = BTreeSet::new();
    for (path, item) in paths {
        let item = item.as_object().ok_or("path item is not an object")?;
        for m in HTTP_METHODS {
            let Some(op) = item.get(*m).and_then(Value::as_object) else {
                continue;
            };
            let where_ = format!("{} {path}", m.to_uppercase());
            let id = op
                .get("operationId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("{where_}: missing operationId"))?;
            let tags = op
                .get("tags")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let tag = match tags.as_slice() {
                [Value::String(t)] => t.clone(),
                _ => {
                    return Err(format!(
                        "{where_} ({id}): needs exactly one tag, has {}",
                        tags.len()
                    ))
                }
            };
            if !is_name(&tag) {
                return Err(format!("{where_}: tag \"{tag}\" is not snake_case"));
            }
            let action = id
                .strip_prefix(&format!("{tag}_"))
                .filter(|a| is_name(a))
                .ok_or_else(|| {
                    format!("{where_}: operationId \"{id}\" must be \"{tag}_<snake_case action>\"")
                })?
                .to_string();
            if !seen.insert(id.to_string()) {
                return Err(format!("{where_}: duplicate operationId \"{id}\""));
            }
            if op.get("x-infi-facade").and_then(Value::as_bool) != Some(true) {
                return Err(format!("{where_} ({id}): missing x-infi-facade: true"));
            }
            let scope = op
                .get("x-infi-scope")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("{where_} ({id}): missing x-infi-scope"))?
                .to_string();
            ops.push(Op {
                operation_id: id.to_string(),
                tag,
                action,
                method: m.to_uppercase(),
                path: path.clone(),
                scope,
                summary: doc_line(op.get("summary").and_then(Value::as_str).unwrap_or("")),
                raw: op.clone(),
                item: item.clone(),
            });
        }
    }
    if ops.is_empty() {
        return Err("contract without operations".into());
    }
    Ok(ops)
}

pub struct Param {
    name: String,
    field: String,
    rust: &'static str,
    required: bool,
}

impl Param {
    fn field_type(&self) -> String {
        if self.required {
            self.rust.to_string()
        } else {
            format!("Option<{}>", self.rust)
        }
    }
}

pub struct BuiltOp {
    op: Op,
    prefix: String,
    path: Vec<Param>,
    query: Vec<Param>,
    headers: Vec<Param>,
    body: Option<(String, bool)>,
    response: Option<String>,
    response_note: Option<String>,
}

pub struct Built {
    tags: Vec<String>,
    ops: Vec<BuiltOp>,
}

fn deref<'a>(doc: &'a Value, v: &'a Value) -> Result<(&'a Value, Option<String>), String> {
    let mut cur = v;
    let mut name = None;
    for _ in 0..16 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return Ok((cur, name));
        };
        let parts: Vec<&str> = r.split('/').collect();
        if parts.len() != 4 || parts[0] != "#" || parts[1] != "components" {
            return Err(format!("unsupported $ref: {r}"));
        }
        cur = doc
            .pointer(&format!("/components/{}/{}", parts[2], parts[3]))
            .ok_or_else(|| format!("$ref without target: {r}"))?;
        name = Some(parts[3].to_string());
    }
    Err("$ref cycle".into())
}

fn primitive(doc: &Value, schema: &Value) -> Result<&'static str, String> {
    let (s, _) = deref(doc, schema)?;
    let t = match s.get("type") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .map(str::to_string),
        _ => None,
    };
    match t.as_deref() {
        Some("string") => Ok("String"),
        Some("integer") => Ok("i64"),
        Some("number") => Ok("f64"),
        Some("boolean") => Ok("bool"),
        other => Err(format!("unsupported parameter type: {other:?}")),
    }
}

type Params = (Vec<Param>, Vec<Param>, Vec<Param>);

fn collect_params(doc: &Value, op: &Op) -> Result<Params, String> {
    let mut order: Vec<(String, String)> = vec![];
    let mut by_key: std::collections::BTreeMap<(String, String), Value> = Default::default();
    for list in [op.item.get("parameters"), op.raw.get("parameters")] {
        for it in list.and_then(Value::as_array).into_iter().flatten() {
            let (p, _) = deref(doc, it)?;
            let name = p.get("name").and_then(Value::as_str).unwrap_or_default();
            let in_ = p.get("in").and_then(Value::as_str).unwrap_or_default();
            let key_name = if in_ == "header" {
                name.to_lowercase()
            } else {
                name.to_string()
            };
            let key = (in_.to_string(), key_name);
            if !by_key.contains_key(&key) {
                order.push(key.clone());
            }
            by_key.insert(key, p.clone()); // the operation's parameter wins over the path item's
        }
    }
    let (mut path, mut query, mut headers) = (vec![], vec![], vec![]);
    for key in order {
        let p = &by_key[&key];
        let name = p["name"].as_str().unwrap_or_default();
        let in_ = key.0.as_str();
        if in_ == "cookie" {
            return Err(format!(
                "{} {} ({}): cookie parameters are not supported",
                op.method, op.path, op.operation_id
            ));
        }
        if in_ == "header" && TRANSPORT_HEADERS.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        let rust = primitive(doc, &p["schema"]).map_err(|e| {
            format!(
                "{} {} ({}): parameter \"{name}\": {e}",
                op.method, op.path, op.operation_id
            )
        })?;
        let required = p.get("required").and_then(Value::as_bool).unwrap_or(false) || in_ == "path";
        let param = Param {
            name: name.to_string(),
            field: ident(&snake(name)),
            rust,
            required,
        };
        match in_ {
            "path" => path.push(param),
            "query" => query.push(param),
            "header" => headers.push(param),
            _ => {}
        }
    }
    Ok((path, query, headers))
}

fn json_schema(holder: &Value) -> Option<&Value> {
    holder.pointer("/content/application~1json/schema")
}

pub fn build(doc: &Value, ops: &[Op], types: &mut Types) -> Result<Built, String> {
    let mut tags: Vec<String> = ops.iter().map(|o| o.tag.clone()).collect();
    tags.sort();
    tags.dedup();
    let mut out = vec![];
    let mut taken = BTreeSet::new();
    for o in ops {
        let where_ = format!("{} {} ({})", o.method, o.path, o.operation_id);
        let prefix = pascal(&o.operation_id);
        let (path, query, headers) = collect_params(doc, o)?;

        let mut body = None;
        if let Some(rb) = o.raw.get("requestBody") {
            let (rb, _) = deref(doc, rb)?;
            let schema = json_schema(rb)
                .ok_or_else(|| format!("{where_}: requestBody without application/json"))?;
            let required = rb.get("required").and_then(Value::as_bool).unwrap_or(false);
            let ty = body_type(types, schema, &format!("{prefix}Request"))?;
            body = Some((ty, required));
        }

        // Response: the 2xx with a JSON body. With several differing 2xx schemas the
        // highest status wins (the contract declares it as the superset: 202 of
        // config_apply = result + previewUrl).
        let responses = o.raw.get("responses").and_then(Value::as_object);
        let mut statuses: Vec<&String> = responses
            .into_iter()
            .flat_map(|r| r.keys())
            .filter(|s| s.len() == 3 && s.starts_with('2'))
            .collect();
        statuses.sort();
        if statuses.is_empty() {
            return Err(format!("{where_}: no 2xx response"));
        }
        let mut found: Vec<String> = vec![];
        for s in statuses {
            let (r, name) = deref(doc, &responses.unwrap()[s])?;
            let Some(schema) = json_schema(r) else {
                continue;
            };
            let ty = match (schema.get("$ref"), name) {
                (Some(_), _) => body_type(types, schema, "")?,
                // Inline schema inside components/responses: named after the response.
                (None, Some(n)) => {
                    types.define_inline(&format!("{}Response", pascal(&n)), schema)?
                }
                (None, None) => types.define_inline(&format!("{prefix}Response"), schema)?,
            };
            found.push(ty);
        }
        let response = found.last().cloned();
        let response_note = if found.iter().any(|t| Some(t) != response.as_ref()) {
            Some(found.join(" | "))
        } else {
            None
        };

        let b = BuiltOp {
            op: Op {
                operation_id: o.operation_id.clone(),
                tag: o.tag.clone(),
                action: o.action.clone(),
                method: o.method.clone(),
                path: o.path.clone(),
                scope: o.scope.clone(),
                summary: o.summary.clone(),
                raw: Obj::new(),
                item: Obj::new(),
            },
            prefix: prefix.clone(),
            path,
            query,
            headers,
            body,
            response,
            response_note,
        };
        for suffix in ["Input", "Path", "Query", "Headers"] {
            let n = format!("{prefix}{suffix}");
            if !taken.insert(n.clone()) {
                return Err(format!("{where_}: generated name {n} collides"));
            }
        }
        out.push(b);
    }
    Ok(Built { tags, ops: out })
}

fn body_type(types: &mut Types, schema: &Value, inline: &str) -> Result<String, String> {
    if schema.get("$ref").is_some() {
        return Ok(types.build(schema, inline, None)?.rust);
    }
    types.define_inline(inline, schema)
}

pub fn render(b: &Built) -> String {
    let mut o = String::from(
        "// Code generated by `cargo xtask generate` from api/openapi.sdk.json. DO NOT EDIT.\n\
         \n\
         //! One service per contract tag, one method per operation.\n\
         \n\
         #![allow(clippy::all, unused_imports, missing_docs)]\n\
         \n\
         use crate::client::{CallOptions, Client, Request};\n\
         use crate::error::Error;\n\
         use crate::generated::types::*;\n\n",
    );

    o.push_str("impl Client {\n");
    for t in &b.tags {
        o.push_str(&format!(
            "    /// Operations of the `{t}` tag.\n    pub fn {}(&self) -> {}Service {{\n        {}Service {{ c: self.clone() }}\n    }}\n\n",
            ident(t),
            pascal(t),
            pascal(t)
        ));
    }
    o.push_str("}\n\n");

    for t in &b.tags {
        let n = pascal(t);
        o.push_str(&format!(
            "/// Operations of the `{t}` tag (`client.{}()`).\n#[derive(Debug, Clone)]\npub struct {n}Service {{\n    c: Client,\n}}\n\n",
            ident(t)
        ));
    }

    for op in &b.ops {
        let p = &op.prefix;
        let has_input = !op.path.is_empty()
            || !op.query.is_empty()
            || !op.headers.is_empty()
            || op.body.is_some();
        let parts: Vec<(&str, &Vec<Param>, &str)> = vec![
            ("path", &op.path, "path"),
            ("query", &op.query, "query"),
            ("headers", &op.headers, "headers"),
        ];
        for (field, params, what) in &parts {
            if params.is_empty() {
                continue;
            }
            let ty = pascal(field);
            let ty = if *field == "headers" {
                "Headers".to_string()
            } else {
                ty
            };
            o.push_str(&format!(
                "/// The {what} parameters of `{}`{}.\n#[derive(Debug, Clone, Default, PartialEq)]\npub struct {p}{ty} {{\n",
                op.op.operation_id,
                if *field == "headers" {
                    " (Idempotency-Key, If-Match and X-Infi-Channel are call options)"
                } else {
                    ""
                }
            ));
            for q in params.iter() {
                o.push_str(&format!(
                    "    /// `{}`\n    pub {}: {},\n",
                    q.name,
                    q.field,
                    q.field_type()
                ));
            }
            o.push_str("}\n\n");
        }
        if has_input {
            o.push_str(&format!(
                "/// The parameters of `{}`.\n#[derive(Debug, Clone, Default, PartialEq)]\npub struct {p}Input {{\n",
                op.op.operation_id
            ));
            if !op.path.is_empty() {
                o.push_str(&format!("    pub path: {p}Path,\n"));
            }
            if !op.query.is_empty() {
                o.push_str(&format!("    pub query: {p}Query,\n"));
            }
            if !op.headers.is_empty() {
                o.push_str(&format!("    pub headers: {p}Headers,\n"));
            }
            if let Some((ty, req)) = &op.body {
                if *req {
                    o.push_str(&format!("    pub body: {ty},\n"));
                } else {
                    o.push_str(&format!("    pub body: Option<{ty}>,\n"));
                }
            }
            o.push_str("}\n\n");
        }

        let svc = format!("{}Service", pascal(&op.op.tag));
        let name = ident(&op.op.action);
        let with = ident(&format!("{}_with", op.op.action));
        let mut doc = format!("{} {} (scope {})", op.op.method, op.op.path, op.op.scope);
        if !op.op.summary.is_empty() {
            doc.push_str(&format!(": {}", op.op.summary));
        }
        let mut note = String::new();
        if let (Some(n), Some(r)) = (&op.response_note, &op.response) {
            note = format!(
                "    ///\n    /// 2xx responses with different schemas ({n}): the response is decoded into `{r}`.\n"
            );
        }
        let ret = match &op.response {
            Some(r) => r.clone(),
            None => "()".to_string(),
        };
        let (arg, pass) = if has_input {
            (format!(", input: {p}Input"), "input, ")
        } else {
            (String::new(), "")
        };
        o.push_str(&format!("impl {svc} {{\n"));
        o.push_str(&format!(
            "    /// {doc}\n{note}    pub async fn {name}(&self{arg}) -> Result<{ret}, Error> {{\n        self.{with}({pass}CallOptions::default()).await\n    }}\n\n"
        ));
        o.push_str(&format!(
            "    /// Like [`{}`](Self::{}), with call options (idempotency key, `If-Match`, channel).\n    pub async fn {with}(&self{arg}, opts: CallOptions) -> Result<{ret}, Error> {{\n",
            op.op.action, op.op.action
        ));
        let mutable = has_input;
        o.push_str(&format!(
            "        let {}req = Request::new({:?}, {:?});\n",
            if mutable { "mut " } else { "" },
            op.op.method,
            op.op.path
        ));
        for q in &op.path {
            o.push_str(&format!(
                "        req.path_param({:?}, input.path.{});\n",
                q.name, q.field
            ));
        }
        for (list, field, method) in [
            (&op.query, "query", "query"),
            (&op.headers, "headers", "header"),
        ] {
            for q in list {
                if q.required {
                    o.push_str(&format!(
                        "        req.{method}({:?}, input.{field}.{});\n",
                        q.name, q.field
                    ));
                } else {
                    o.push_str(&format!(
                        "        if let Some(v) = input.{field}.{} {{\n            req.{method}({:?}, v);\n        }}\n",
                        q.field, q.name
                    ));
                }
            }
        }
        if let Some((_, req)) = &op.body {
            if *req {
                o.push_str("        req.json_body(&input.body)?;\n");
            } else {
                o.push_str("        if let Some(b) = &input.body {\n            req.json_body(b)?;\n        }\n");
            }
        }
        if op.response.is_some() {
            o.push_str("        self.c.execute(req, &opts).await\n");
        } else {
            o.push_str("        self.c.execute_unit(req, &opts).await\n");
        }
        o.push_str("    }\n}\n\n");
    }

    o.push_str(
        "/// An operation of the contract, for the reachability test.\n#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Operation {\n    pub operation_id: &'static str,\n    pub tag: &'static str,\n    /// The method on `client.<tag>()`.\n    pub method: &'static str,\n    pub http_method: &'static str,\n    pub path: &'static str,\n    pub scope: &'static str,\n}\n\n",
    );
    o.push_str("/// The operations of the contract.\npub const OPERATIONS: &[Operation] = &[\n");
    for op in &b.ops {
        o.push_str(&format!(
            "    Operation {{ operation_id: {:?}, tag: {:?}, method: {:?}, http_method: {:?}, path: {:?}, scope: {:?} }},\n",
            op.op.operation_id, op.op.tag, op.op.action, op.op.method, op.op.path, op.op.scope
        ));
    }
    o.push_str("];\n");
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(over: Value) -> Value {
        let mut o = json!({
            "operationId": "payments_list",
            "tags": ["payments"],
            "x-infi-facade": true,
            "x-infi-scope": "payments:read",
            "responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/PaymentList"}}}}},
        });
        for (k, v) in over.as_object().unwrap() {
            if v.is_null() {
                o.as_object_mut().unwrap().remove(k);
            } else {
                o[k] = v.clone();
            }
        }
        o
    }

    fn doc(o: Value) -> Value {
        json!({"paths": {"/v1/payments": {"get": o}}, "components": {"schemas": {}}})
    }

    fn want_err(d: &Value, sub: &str) {
        match collect_ops(d) {
            Err(e) => assert!(e.contains(sub), "{e} does not contain {sub}"),
            Ok(_) => panic!("expected an error with {sub}"),
        }
    }

    #[test]
    fn accepts_a_valid_operation_and_names_it() {
        let d = doc(op(
            json!({"operationId": "customers_get_wallet", "tags": ["customers"]}),
        ));
        let ops = collect_ops(&d).unwrap();
        assert_eq!(ops[0].action, "get_wallet");
        let mut types = Types::new(&d);
        let built = build(&d, &ops, &mut types).unwrap();
        let src = render(&built);
        assert!(src.contains("pub async fn get_wallet("));
        assert!(src.starts_with("// Code generated by"));
        assert!(src.contains("DO NOT EDIT."));
    }

    #[test]
    fn rejects_bad_operation_ids_tags_and_markers() {
        want_err(
            &doc(op(json!({"operationId": "listPayments"}))),
            "payments_<snake_case action>",
        );
        want_err(&doc(op(json!({"x-infi-scope": null}))), "x-infi-scope");
        want_err(
            &doc(op(json!({"tags": ["payments", "usage"]}))),
            "exactly one tag",
        );
        want_err(&doc(op(json!({"tags": ["Payments"]}))), "not snake_case");
        want_err(
            &doc(op(json!({"operationId": null}))),
            "missing operationId",
        );
        want_err(&doc(op(json!({"x-infi-facade": null}))), "x-infi-facade");
        want_err(&json!({"paths": {}}), "without operations");
        let dup = json!({"paths": {"/a": {"get": op(json!({}))}, "/b": {"get": op(json!({}))}}});
        want_err(&dup, "duplicate");
    }

    #[test]
    fn several_2xx_schemas_use_the_highest_status() {
        let two = json!({
            "200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/A"}}}},
            "202": {"content": {"application/json": {"schema": {"properties": {}}}}},
        });
        let d = doc(op(json!({"responses": two})));
        let ops = collect_ops(&d).unwrap();
        let mut types = Types::new(&d);
        let built = build(&d, &ops, &mut types).unwrap();
        assert_eq!(
            built.ops[0].response.as_deref(),
            Some("PaymentsListResponse")
        );
        assert!(built.ops[0].response_note.is_some());
    }

    #[test]
    fn path_item_and_operation_parameters_and_transport_headers() {
        let d = json!({"components": {"schemas": {}}, "paths": {"/v1/x/{id}": {
            "parameters": [{"in": "path", "name": "id", "required": true, "schema": {"type": "string"}}],
            "post": op(json!({
                "operationId": "payments_create", "tags": ["payments"],
                "parameters": [
                    {"in": "header", "name": "Idempotency-Key", "schema": {"type": "string"}},
                    {"in": "header", "name": "If-Match", "required": true, "schema": {"type": "string"}},
                    {"in": "header", "name": "Accept-Language", "schema": {"type": "string"}},
                    {"in": "query", "name": "limit", "schema": {"type": "integer"}},
                ],
            })),
        }}});
        let ops = collect_ops(&d).unwrap();
        let (path, query, headers) = collect_params(&d, &ops[0]).unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].field_type(), "String");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "Accept-Language");
        assert_eq!(headers[0].field_type(), "Option<String>");
        assert_eq!(query[0].field_type(), "Option<i64>");
    }

    #[test]
    fn naming_helpers() {
        assert_eq!(pascal("customer_sessions"), "CustomerSessions");
        assert_eq!(snake("XBuyerToken"), "x_buyer_token");
        assert_eq!(snake("enrollmentId"), "enrollment_id");
        assert_eq!(snake("X-Buyer-Token"), "x_buyer_token");
        assert_eq!(ident("type"), "r#type");
    }
}
