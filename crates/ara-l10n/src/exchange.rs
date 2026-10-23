use crate::{ParamKind, Provenance, Result, Status, Table, TableCommand, error};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Translator-facing unit; ICU patterns stay whole, including plural/select branches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ExchangeUnit {
    /// Stable message key, never the source sentence.
    pub key: String,
    /// Source ICU message, used to detect outdated translator files.
    pub source: String,
    /// Translated ICU message, or empty for an untranslated unit.
    pub target: String,
    /// Parameter signature carried losslessly in Ara metadata.
    pub params: BTreeMap<String, ParamKind>,
    /// Human/machine review state.
    pub status: Status,
    /// Optional generation provenance.
    pub provenance: Option<Provenance>,
    /// Translator context.
    pub note: String,
}

fn units(source: &Table, target: &Table) -> Vec<ExchangeUnit> {
    source
        .entries()
        .map(|entry| {
            let translated = target.entry(&entry.key);
            ExchangeUnit {
                key: entry.key.clone(),
                source: entry.text.clone(),
                target: translated.map_or(String::new(), |e| e.text.clone()),
                params: entry.params.clone(),
                status: translated.map_or(Status::NeedsReview, |e| e.status),
                provenance: translated.and_then(|e| e.provenance.clone()),
                note: entry.note.clone(),
            }
        })
        .collect()
}

fn metadata(unit: &ExchangeUnit) -> Result<String> {
    let mut unit = unit.clone();
    unit.source.clear();
    unit.target.clear();
    Ok(ara_json::to_canonical_string(&ara_json::sort_keys(
        &serde_json::to_value(unit).map_err(|e| error(e.to_string()))?,
    )))
}
fn xml(text: &str) -> String {
    quick_xml::escape::escape(text).into_owned()
}

/// Writes standards-based XLIFF 2.0 with whole ICU patterns and Ara review metadata.
pub fn export_xliff(source: &Table, target: &Table) -> Result<String> {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xliff xmlns=\"urn:oasis:names:tc:xliff:document:2.0\" version=\"2.0\" srcLang=\"{}\" trgLang=\"{}\"><file id=\"ara\">\n",
        xml(&source.config().locale),
        xml(&target.config().locale)
    );
    for unit in units(source, target) {
        let state = if unit.status == Status::Reviewed {
            "final"
        } else {
            "translated"
        };
        out.push_str(&format!("<unit id=\"{}\"><notes><note category=\"ara\">{}</note></notes><segment state=\"{state}\"><source>{}</source><target>{}</target></segment></unit>\n", xml(&unit.key), xml(&metadata(&unit)?), xml(&unit.source), xml(&unit.target)));
    }
    out.push_str("</file></xliff>\n");
    Ok(out)
}

/// Reads XLIFF 2.0 plain-text ICU units. DTDs, inline markup and unknown entities
/// are rejected so translator input cannot silently lose or reinterpret text.
pub fn import_xliff(text: &str) -> Result<Vec<ExchangeUnit>> {
    if text.len() > 16 * 1024 * 1024 {
        return Err(error("XLIFF exceeds 16 MiB"));
    }
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<ExchangeUnit> = None;
    let mut meta = String::new();
    let mut state = String::new();
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        match reader.read_event().map_err(|e| error(e.to_string()))? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if !matches!(
                    name.as_str(),
                    "xliff" | "file" | "unit" | "notes" | "note" | "segment" | "source" | "target"
                ) {
                    return Err(error(format!("unsupported XLIFF element {name}")));
                }
                if stack
                    .last()
                    .is_some_and(|n| matches!(n.as_str(), "source" | "target" | "note"))
                {
                    return Err(error(
                        "inline XLIFF markup requires conversion to ICU text first",
                    ));
                }
                if name == "unit" {
                    if current.is_some() {
                        return Err(error("nested XLIFF unit"));
                    }
                    let key =
                        attribute(&e, b"id")?.ok_or_else(|| error("XLIFF unit requires id"))?;
                    if !seen.insert(key.clone()) {
                        return Err(error("duplicate XLIFF unit"));
                    }
                    current = Some(ExchangeUnit {
                        key,
                        ..Default::default()
                    });
                    meta.clear();
                    state.clear();
                }
                if name == "segment" {
                    state = attribute(&e, b"state")?.unwrap_or_default();
                }
                stack.push(name);
            }
            Event::Text(e) => {
                let decoded = e.decode().map_err(|e| error(e.to_string()))?;
                append_text(&stack, &mut current, &mut meta, &decoded);
            }
            Event::CData(e) => append_text(
                &stack,
                &mut current,
                &mut meta,
                &e.decode().map_err(|e| error(e.to_string()))?,
            ),
            Event::GeneralRef(e) => {
                let name = e.decode().map_err(|e| error(e.to_string()))?;
                let decoded = quick_xml::escape::unescape(&format!("&{name};"))
                    .map_err(|e| error(e.to_string()))?
                    .into_owned();
                append_text(&stack, &mut current, &mut meta, &decoded);
            }
            Event::End(_) => {
                let name = stack.pop().ok_or_else(|| error("unexpected XLIFF close"))?;
                if name == "unit" {
                    let mut unit = current.take().ok_or_else(|| error("missing XLIFF unit"))?;
                    if !meta.is_empty() {
                        let data: ExchangeUnit = serde_json::from_value(
                            ara_json::parse(&meta).map_err(|e| error(e.to_string()))?,
                        )
                        .map_err(|e| error(e.to_string()))?;
                        if data.key != unit.key {
                            return Err(error("XLIFF metadata key mismatch"));
                        }
                        unit.params = data.params;
                        unit.status = data.status;
                        unit.provenance = data.provenance;
                        unit.note = data.note;
                    }
                    if state == "final" || state == "reviewed" {
                        unit.status = Status::Reviewed;
                    }
                    output.push(unit);
                }
            }
            Event::Empty(e) if matches!(e.local_name().as_ref(), b"source" | b"target") => {}
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Eof => break,
            _ => return Err(error("unsupported XLIFF declaration or element")),
        }
    }
    if !stack.is_empty() || current.is_some() {
        return Err(error("truncated XLIFF"));
    }
    Ok(output)
}

fn attribute(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in e.attributes() {
        let attribute = attribute.map_err(|e| error(e.to_string()))?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                attribute
                    .unescape_value()
                    .map_err(|e| error(e.to_string()))?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}
fn append_text(
    stack: &[String],
    current: &mut Option<ExchangeUnit>,
    meta: &mut String,
    text: &str,
) {
    if let Some(unit) = current {
        match stack.last().map(String::as_str) {
            Some("source") => unit.source.push_str(text),
            Some("target") => unit.target.push_str(text),
            Some("note") => meta.push_str(text),
            _ => {}
        }
    }
}

/// Exports gettext PO: keys in msgctxt, complete ICU patterns in msgid/msgstr.
/// ICU plural branches stay together; gettext's positional plural slots are not used.
pub fn export_po(source: &Table, target: &Table) -> Result<String> {
    let header = format!(
        "Language: {}\nContent-Type: text/plain; charset=UTF-8\n",
        target.config().locale
    );
    let mut out = format!("msgid \"\"\nmsgstr {}\n\n", quote(&header)?);
    for unit in units(source, target) {
        let meta = metadata(&unit)?.replace('\n', "");
        out.push_str(&format!(
            "#. ara: {meta}\n{}msgctxt {}\nmsgid {}\nmsgstr {}\n\n",
            if unit.status == Status::Reviewed {
                ""
            } else {
                "#, fuzzy\n"
            },
            quote(&unit.key)?,
            quote(&unit.source)?,
            quote(&unit.target)?
        ));
    }
    Ok(out)
}
fn quote(text: &str) -> Result<String> {
    serde_json::to_string(text).map_err(|e| error(e.to_string()))
}

/// Imports UTF-8 PO including multiline strings and fuzzy review status.
/// Native gettext plural arrays are rejected: supply the complete ICU pattern.
pub fn import_po(text: &str) -> Result<Vec<ExchangeUnit>> {
    if text.len() > 16 * 1024 * 1024 {
        return Err(error("PO exceeds 16 MiB"));
    }
    let mut output = Vec::new();
    let mut unit = ExchangeUnit::default();
    let mut field = "";
    let mut fuzzy = false;
    let mut seen = BTreeSet::new();
    let mut dirty = false;
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim();
        if line.is_empty() {
            if dirty && !unit.key.is_empty() {
                if !seen.insert(unit.key.clone()) {
                    return Err(error("duplicate PO key"));
                }
                if !fuzzy {
                    unit.status = Status::Reviewed;
                } else if unit.status == Status::Reviewed {
                    unit.status = Status::NeedsReview;
                }
                output.push(std::mem::take(&mut unit));
            }
            unit = ExchangeUnit::default();
            field = "";
            fuzzy = false;
            dirty = false;
            continue;
        }
        if let Some(meta) = line.strip_prefix("#. ara: ") {
            unit = serde_json::from_value(ara_json::parse(meta).map_err(|e| error(e.to_string()))?)
                .map_err(|e| error(e.to_string()))?;
            unit.key.clear();
            unit.source.clear();
            unit.target.clear();
            continue;
        }
        if line.starts_with("#,") {
            fuzzy |= line.split([',', ' ']).any(|s| s == "fuzzy");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let value = if let Some(value) = line.strip_prefix("msgctxt ") {
            field = "key";
            value
        } else if let Some(value) = line.strip_prefix("msgid ") {
            field = "source";
            value
        } else if let Some(value) = line.strip_prefix("msgstr ") {
            field = "target";
            value
        } else if line.starts_with('"') && !field.is_empty() {
            line
        } else {
            return Err(error("unsupported PO directive; use whole ICU patterns"));
        };
        let value: String = serde_json::from_str(value).map_err(|e| error(e.to_string()))?;
        match field {
            "key" => unit.key.push_str(&value),
            "source" => unit.source.push_str(&value),
            "target" => unit.target.push_str(&value),
            _ => {}
        }
        dirty = true;
    }
    Ok(output)
}

/// Validates translator input against the current source and returns typed commands.
/// Apply all commands in one transaction; stale files never overwrite source edits.
pub fn import_commands(
    source: &Table,
    target: &Table,
    units: Vec<ExchangeUnit>,
) -> Result<Vec<TableCommand>> {
    let mut commands = Vec::new();
    let mut seen = BTreeSet::new();
    for unit in units {
        if !seen.insert(unit.key.clone()) {
            return Err(error("duplicate imported key"));
        }
        let original = source
            .entry(&unit.key)
            .ok_or_else(|| error("translator file contains an unknown key"))?;
        if original.text != unit.source
            || (!unit.params.is_empty() && unit.params != original.params)
        {
            return Err(error(format!("stale source/signature for {}", unit.key)));
        }
        if unit.target.is_empty() {
            continue;
        }
        let mut entry = original.clone();
        entry.text = unit.target;
        entry.status = unit.status;
        entry.provenance = unit.provenance;
        crate::Message::parse(&entry.text, &entry.params)?;
        let id = if let Some((id, _)) = target.entries.get(&entry.key) {
            id.clone()
        } else {
            crate::tools::record_id(&entry.key)?
        };
        commands.push(TableCommand::Put { id, entry });
    }
    Ok(commands)
}
