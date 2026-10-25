//! Typed editors for compound Inspector fields: lists, keyed lists, maps, optional and
//! nested structs. Each element gets the widget its schema type asks for, and every
//! edit replaces the whole field value with one `property.set` per selected entity
//! (`list<T>` is never addressed by index). An element is addressed `path#pointer`, a
//! JSON pointer into the field's value: `field.set`, typed drafts (`@nested|…`), the
//! asset picker and asset drops all take that target.
use super::inspector_panel::sentence;
use super::panels::display;
use super::*;
use crate::inspector::Field;
use ara_schema::{FieldDesc, SchemaDb, Shape, ValueType};

/// Items listed per list or map; the rest are counted.
const SHOWN: usize = 32;

/// How a nested text box's draft parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Leaf {
    /// A number (an integer when typed as one).
    Number,
    /// A string.
    Text,
    /// `#RRGGBB` sRGB for linear channels; alpha keeps its value.
    Hex,
    /// A map entry's key: renames the entry.
    Key,
    /// Any JSON (`json` values only).
    Json,
}
impl Leaf {
    fn code(self) -> char {
        match self {
            Leaf::Number => 'n',
            Leaf::Text => 't',
            Leaf::Hex => 'h',
            Leaf::Key => 'k',
            Leaf::Json => 'j',
        }
    }
    fn from_code(code: char) -> Option<Self> {
        Some(match code {
            'n' => Leaf::Number,
            't' => Leaf::Text,
            'h' => Leaf::Hex,
            'k' => Leaf::Key,
            'j' => Leaf::Json,
            _ => return None,
        })
    }
    /// The draft key of an edit at `target`; `nullable` makes empty text clear it.
    pub(super) fn draft(self, target: &str, nullable: bool) -> String {
        format!(
            "@nested|{}{}|{target}",
            self.code(),
            if nullable { "?" } else { "" }
        )
    }
}

/// `key` as one JSON-pointer token.
fn token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
/// The tokens of a JSON pointer (`""` is the whole value).
fn tokens(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}
/// Puts `to` at `tokens` inside `value`, creating objects for missing struct fields;
/// an index one past a list's end appends an item.
fn set_at(value: &mut Value, tokens: &[String], to: Value) -> Result<(), String> {
    let Some((first, rest)) = tokens.split_first() else {
        *value = to;
        return Ok(());
    };
    if value.is_null() {
        *value = Value::Object(Default::default());
    }
    let child = match value {
        Value::Object(map) => map.entry(first.clone()).or_insert(Value::Null),
        Value::Array(items) => {
            let i = first
                .parse::<usize>()
                .map_err(|_| format!("`{first}` is not an item"))?;
            if i == items.len() {
                items.push(Value::Null);
            }
            items
                .get_mut(i)
                .ok_or_else(|| format!("item {first} no longer exists"))?
        }
        _ => return Err(format!("`{first}` is not inside this value")),
    };
    set_at(child, rest, to)
}
/// `#RRGGBB` (sRGB) as linear channels, rounded for stable documents.
pub(super) fn hex_linear(text: &str) -> Result<[f64; 3], String> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return Err("Enter a colour as #RRGGBB".into());
    }
    let channel = |n: usize| -> Result<f64, String> {
        let byte = u8::from_str_radix(&hex[n * 2..n * 2 + 2], 16)
            .map_err(|_| "Enter a colour as #RRGGBB".to_owned())?;
        Ok(((byte as f64 / 255.).powf(2.2) * 10000.).round() / 10000.)
    };
    Ok([channel(0)?, channel(1)?, channel(2)?])
}
/// Linear channels as `#RRGGBB` sRGB.
fn linear_hex(value: &Value) -> String {
    let channel = |n: usize| {
        (value[n].as_f64().unwrap_or(1.).clamp(0., 1.).powf(1. / 2.2) * 255.).round() as u8
    };
    format!("#{:02X}{:02X}{:02X}", channel(0), channel(1), channel(2))
}
/// Applies one typed draft at `pointer` inside a field's whole value.
fn apply_draft(whole: &mut Value, pointer: &str, code: &str, text: &str) -> Result<(), String> {
    let mut chars = code.chars();
    let leaf = chars
        .next()
        .and_then(Leaf::from_code)
        .ok_or("invalid nested edit")?;
    let nullable = chars.next() == Some('?');
    let at = tokens(pointer);
    if nullable && text.trim().is_empty() {
        return set_at(whole, &at, Value::Null);
    }
    let value = match leaf {
        Leaf::Number => serde_json::from_str::<Value>(text.trim())
            .ok()
            .filter(Value::is_number)
            .map_or_else(
                || crate::inspector::parse_number(text, None).map(|n| json!(n)),
                Ok,
            )?,
        Leaf::Text => json!(text),
        Leaf::Json => serde_json::from_str(text).map_err(|e| e.to_string())?,
        Leaf::Hex => {
            let mut rgb: Vec<Value> = hex_linear(text)?.iter().map(|c| json!(c)).collect();
            if let Some(alpha) = whole.pointer(pointer).and_then(|v| v.get(3)) {
                rgb.push(alpha.clone());
            }
            Value::Array(rgb)
        }
        Leaf::Key => {
            let (old, parent) = at.split_last().ok_or("invalid key edit")?;
            let key = text.trim();
            let map = whole
                .pointer_mut(
                    &parent
                        .iter()
                        .map(|t| format!("/{}", token(t)))
                        .collect::<String>(),
                )
                .and_then(Value::as_object_mut)
                .ok_or("the entry no longer exists")?;
            if key == old {
                return Ok(());
            }
            if key.is_empty() {
                return Err("A key cannot be empty".into());
            }
            if map.contains_key(key) {
                return Err(format!("`{key}` already exists"));
            }
            let entry = map.remove(old).ok_or("the entry no longer exists")?;
            map.insert(key.to_owned(), entry);
            return Ok(());
        }
    };
    set_at(whole, &at, value)
}
/// `property.set` commands putting `value` at `pointer` inside `field`.
fn set_commands(selection: &[String], field: &Field, pointer: &str, value: Value) -> Vec<Value> {
    let mut whole = field.value.clone();
    match set_at(&mut whole, &tokens(pointer), value) {
        Ok(()) => crate::inspector::edit_commands(selection, &field.path, &whole),
        Err(_) => Vec::new(),
    }
}
/// A new element of `ty`: a struct with every field at its default, else the zero.
fn element_default(db: &SchemaDb, ty: &ValueType) -> Value {
    if let ValueType::Named(name) = ty
        && let Some(Shape::Struct { fields, .. }) = db.get(name).map(|t| &t.shape)
    {
        return Value::Object(
            fields
                .iter()
                .map(|f| (f.name.clone(), db.field_default(f)))
                .collect(),
        );
    }
    db.zero(ty)
}
/// A struct's required (non-optional) asset field and its kind: such an item is
/// created by picking the asset, never with an empty reference.
fn required_asset(db: &SchemaDb, ty: &ValueType) -> Option<(String, Option<String>)> {
    let ValueType::Named(name) = ty else {
        return None;
    };
    let Some(Shape::Struct { fields, .. }) = db.get(name).map(|t| &t.shape) else {
        return None;
    };
    fields.iter().find_map(|f| match &f.ty {
        ValueType::Asset(kind) => Some((f.name.clone(), kind.clone())),
        _ => None,
    })
}
/// `ty` without its `option<…>`.
fn inner(ty: &ValueType) -> &ValueType {
    match ty {
        ValueType::Option(inner) => inner,
        _ => ty,
    }
}
/// The singular of a collection's name, for new keys and IDs: `parameters` → `parameter`.
fn singular(name: &str) -> String {
    let word = name.rsplit('/').next().unwrap_or(name);
    word.strip_suffix('s').unwrap_or(word).to_lowercase()
}
/// The first unused `{base}{n}` among `taken`.
fn unused(base: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|n| format!("{base}{n}"))
        .find(|k| !taken(k))
        .unwrap_or_else(|| base.to_owned())
}
/// A list item's row title: a keyed item's ID, or a struct's leading scalar field
/// (`Primitive 2`, `Clip walk`), else its position.
fn item_title(db: &SchemaDb, elem: &ValueType, item: &Value, n: usize) -> String {
    if let Some(id) = item.get("id").and_then(Value::as_str) {
        return id.to_owned();
    }
    if let ValueType::Named(name) = elem
        && let Some(Shape::Struct { fields, .. }) = db.get(name).map(|t| &t.shape)
        && let Some(first) = fields
            .iter()
            .find(|f| crate::inspector::shows(db, fields, f, Some(item)))
    {
        let value = item
            .get(&first.name)
            .cloned()
            .unwrap_or_else(|| db.field_default(first));
        if matches!(value, Value::String(_) | Value::Number(_)) {
            return format!("{} {}", sentence(&first.name), display(&value));
        }
    }
    format!("Item {}", n + 1)
}

impl Shell {
    /// The value at a field target: `path`, or `path#pointer` inside a compound field.
    pub(super) fn field_value(&self, target: &str) -> Value {
        let (path, pointer) = target.split_once('#').unwrap_or((target, ""));
        self.fields
            .get(path)
            .and_then(|f| f.value.pointer(pointer).cloned())
            .unwrap_or(Value::Null)
    }
    /// Whether `target` is one past the end of a list: picking there adds an item.
    pub(super) fn appends(&self, target: &str) -> bool {
        let Some((path, pointer)) = target.split_once('#') else {
            return false;
        };
        let value = self.fields.get(path).map(|f| &f.value);
        let mut at = tokens(pointer);
        while let Some(last) = at.pop() {
            let parent = at
                .iter()
                .map(|t| format!("/{}", token(t)))
                .collect::<String>();
            if let Some(Value::Array(items)) = value.and_then(|v| v.pointer(&parent)) {
                return last.parse::<usize>().is_ok_and(|i| i == items.len());
            }
        }
        false
    }
    /// `property.set` commands putting `value` at a field target; a nested target
    /// replaces its field's whole value.
    pub(super) fn field_commands(
        &self,
        selection: &[String],
        target: &str,
        value: Value,
    ) -> ara_ui::Result<Vec<Value>> {
        let Some((path, pointer)) = target.split_once('#') else {
            return Ok(crate::inspector::edit_commands(selection, target, &value));
        };
        let field = self
            .fields
            .get(path)
            .ok_or("that field is no longer shown")?;
        let mut whole = field.value.clone();
        set_at(&mut whole, &tokens(pointer), value)?;
        Ok(crate::inspector::edit_commands(selection, path, &whole))
    }
    /// The commands for the typed nested drafts, which it takes: each field's drafts
    /// apply to its value in turn, committed whole.
    pub(super) fn nested_commands(&mut self, selection: &[String]) -> ara_ui::Result<Vec<Value>> {
        let keys: Vec<String> = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@nested|"))
            .cloned()
            .collect();
        let mut wholes: BTreeMap<String, Value> = BTreeMap::new();
        for key in keys {
            let text = self.drafts.remove(&key).unwrap_or_default();
            let mut parts = key.splitn(3, '|').skip(1);
            let (Some(code), Some(target)) = (parts.next(), parts.next()) else {
                continue;
            };
            let (path, pointer) = target.split_once('#').ok_or("invalid nested edit")?;
            // A field that is no longer shown drops its draft.
            let Some(field) = self.fields.get(path) else {
                continue;
            };
            let whole = wholes
                .entry(path.to_owned())
                .or_insert_with(|| field.value.clone());
            apply_draft(whole, pointer, code, &text)?;
        }
        Ok(wholes
            .iter()
            .flat_map(|(path, whole)| crate::inspector::edit_commands(selection, path, whole))
            .collect())
    }

    /// A compound field's row head: its count (or None) and Add, or Remove.
    pub(super) fn compound_head(
        &mut self,
        editor: &Editor,
        field: &Field,
        id: &str,
        out: &mut String,
    ) {
        let crate::inspector::Widget::Compound { ty, .. } = &field.widget else {
            return;
        };
        if field.mixed {
            out.push_str(&format!(
                "<text id='mixed-{id}' class='caption' text='Mixed: differs between the selection' />"
            ));
            return;
        }
        self.collection_head(editor, field, ty, &field.value, "", id, out);
    }
    /// The rows under a compound field.
    pub(super) fn compound_rows(
        &mut self,
        editor: &Editor,
        field: &Field,
        label: f32,
        out: &mut String,
    ) {
        let crate::inspector::Widget::Compound { ty, .. } = &field.widget else {
            return;
        };
        if !field.mixed {
            self.items(editor, field, ty, &field.value, "", 1, label, out);
        }
    }

    /// Count/None text and the Add or Remove action for a value of `ty` at `pointer`.
    #[allow(clippy::too_many_arguments)]
    fn collection_head(
        &mut self,
        editor: &Editor,
        field: &Field,
        ty: &ValueType,
        value: &Value,
        pointer: &str,
        rid: &str,
        out: &mut String,
    ) {
        let db = editor.host.store().schema();
        let selection = &editor.state.selection;
        let action = |shell: &mut Self, out: &mut String, what: &str, icon: &str, to: Value| {
            let button = format!("{what}-{rid}");
            shell.bindings.insert(
                ara_ui::source::stable_id(&button),
                Binding::Content(
                    set_commands(selection, field, pointer, to),
                    format!(
                        "{} {}",
                        sentence(what),
                        sentence(&field.label).to_lowercase()
                    ),
                ),
            );
            out.push_str(&format!("<button id='{button}' class='ghost icon-button' label='{}' description='{}' action='Dispatch'>{}</button>",sentence(what),sentence(what),icons::icon_in(&button,icon,"icon-sm")));
        };
        match ty {
            ValueType::Option(inner) if value.is_null() => {
                out.push_str(&format!(
                    "<text id='count-{rid}' class='caption fill' style='padding-left:6px' text='None' />"
                ));
                action(self, out, "add", "plus", element_default(db, inner));
            }
            ValueType::Option(inner) => {
                self.collection_head(editor, field, inner, value, pointer, rid, out);
                action(self, out, "remove", "x", Value::Null);
            }
            ValueType::List(elem) | ValueType::Keyed(elem) => {
                let items = value.as_array().cloned().unwrap_or_default();
                let n = items.len();
                out.push_str(&format!(
                    "<text id='count-{rid}' class='caption fill' style='padding-left:6px' text='{n} item{}' />",
                    if n == 1 { "" } else { "s" }
                ));
                // An item that cannot exist without an asset (a material override's
                // material) is added by picking that asset.
                if let Some((name, kind)) = required_asset(db, elem) {
                    let button = format!("add-{rid}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&button),
                        Binding::AssetPicker(
                            format!("{}#{pointer}/{n}/{}", field.path, token(&name)),
                            kind,
                        ),
                    );
                    out.push_str(&format!("<button id='{button}' class='ghost icon-button' label='Add' description='Pick the {} for a new item' action='Dispatch'>{}</button>",escape(&sentence(&name).to_lowercase()),icons::icon_in(&button,"plus","icon-sm")));
                    return;
                }
                let mut item = element_default(db, elem);
                if matches!(ty, ValueType::Keyed(_))
                    && let ValueType::Named(name) = &**elem
                {
                    let base = name
                        .rfind(|c: char| c.is_uppercase())
                        .map_or(name.as_str(), |i| &name[i..])
                        .to_lowercase();
                    item["id"] = json!(unused(&base, |id| {
                        items.iter().any(|i| i["id"].as_str() == Some(id))
                    }));
                }
                let mut grown = items;
                grown.push(item);
                action(self, out, "add", "plus", Value::Array(grown));
            }
            ValueType::Map(elem) => {
                let mut map = value.as_object().cloned().unwrap_or_default();
                let n = map.len();
                out.push_str(&format!(
                    "<text id='count-{rid}' class='caption fill' style='padding-left:6px' text='{n} entr{}' />",
                    if n == 1 { "y" } else { "ies" }
                ));
                let name = if pointer.is_empty() {
                    field.label.as_str()
                } else {
                    pointer
                };
                let key = unused(&singular(name), |k| map.contains_key(k));
                map.insert(key, element_default(db, elem));
                action(self, out, "add", "plus", Value::Object(map));
            }
            _ => {}
        }
    }

    /// The rows under a compound value: one per struct field, list item or map entry.
    #[allow(clippy::too_many_arguments)]
    fn items(
        &mut self,
        editor: &Editor,
        field: &Field,
        ty: &ValueType,
        value: &Value,
        pointer: &str,
        depth: usize,
        label: f32,
        out: &mut String,
    ) {
        let db = editor.host.store().schema();
        let selection = &editor.state.selection;
        let at = |pointer: &str, to: Value| set_commands(selection, field, pointer, to);
        match ty {
            ValueType::Option(inner) if !value.is_null() => {
                self.items(editor, field, inner, value, pointer, depth, label, out);
            }
            ValueType::Named(name) => {
                let Some(Shape::Struct { fields, .. }) = db.get(name).map(|t| &t.shape) else {
                    return;
                };
                let shown = |f: &&FieldDesc| crate::inspector::shows(db, fields, f, Some(value));
                for desc in fields.iter().filter(shown) {
                    let child = value
                        .get(&desc.name)
                        .cloned()
                        .unwrap_or_else(|| db.field_default(desc));
                    let pointer = format!("{pointer}/{}", token(&desc.name));
                    let row = Row {
                        title: Title::Text(sentence(&desc.name)),
                        ty: &desc.ty,
                        desc: Some(desc),
                        value: &child,
                        pointer: &pointer,
                        actions: Vec::new(),
                        item: false,
                    };
                    self.element(editor, field, row, depth, label, out);
                }
            }
            ValueType::List(elem) | ValueType::Keyed(elem) => {
                let items = value.as_array().cloned().unwrap_or_default();
                for (n, item) in items.iter().enumerate().take(SHOWN) {
                    let mut actions = Vec::new();
                    if n > 0 {
                        let mut moved = items.clone();
                        moved.swap(n - 1, n);
                        actions.push((
                            "up",
                            "chevron-up",
                            "Move up",
                            at(pointer, Value::Array(moved)),
                        ));
                    }
                    let mut removed = items.clone();
                    removed.remove(n);
                    actions.push(("remove", "x", "Remove", at(pointer, Value::Array(removed))));
                    let item_pointer = format!("{pointer}/{n}");
                    let row = Row {
                        title: Title::Text(item_title(db, elem, item, n)),
                        ty: elem,
                        desc: None,
                        value: item,
                        pointer: &item_pointer,
                        actions,
                        item: true,
                    };
                    self.element(editor, field, row, depth, label, out);
                }
                self.more(items.len(), pointer, field, depth, label, out);
            }
            ValueType::Map(elem) => {
                let map = value.as_object().cloned().unwrap_or_default();
                for (key, item) in map.iter().take(SHOWN) {
                    let mut removed = map.clone();
                    removed.remove(key);
                    let actions =
                        vec![("remove", "x", "Remove", at(pointer, Value::Object(removed)))];
                    let item_pointer = format!("{pointer}/{}", token(key));
                    let row = Row {
                        title: Title::Key(key.clone()),
                        ty: elem,
                        desc: None,
                        value: item,
                        pointer: &item_pointer,
                        actions,
                        item: true,
                    };
                    self.element(editor, field, row, depth, label, out);
                }
                self.more(map.len(), pointer, field, depth, label, out);
            }
            _ => {}
        }
    }

    /// "N more" under a list or map longer than [`SHOWN`].
    fn more(
        &mut self,
        count: usize,
        pointer: &str,
        field: &Field,
        depth: usize,
        label: f32,
        out: &mut String,
    ) {
        if count > SHOWN {
            let rid = row_id(field, pointer);
            out.push_str(&format!("<panel id='more-{rid}' class='row' style='padding-left:{}px;min-height:18px'><text id='more-{rid}-text' class='caption' text='{} more' /></panel>",label.min(12. * depth as f32 + 18.),count - SHOWN));
        }
    }

    /// One element row: its title (or editable key), its widget or count, and its
    /// actions; a compound element's own rows follow, one level deeper.
    fn element(
        &mut self,
        editor: &Editor,
        field: &Field,
        row: Row<'_>,
        depth: usize,
        label: f32,
        out: &mut String,
    ) {
        let db = editor.host.store().schema();
        let rid = row_id(field, row.pointer);
        let target = format!("{}#{}", field.path, row.pointer);
        let indent = 6. + 12. * depth as f32;
        out.push_str(&format!("<panel id='nest-{rid}' class='prop-row'><panel id='nest-label-{rid}' class='prop-label' style='width:{label}px;padding-left:{indent}px'>"));
        match &row.title {
            Title::Text(text) => out.push_str(&format!(
                "<text id='nest-title-{rid}' text='{}' />",
                escape(text)
            )),
            Title::Key(key) => {
                let id = format!("nest-key-{rid}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Nested(target.clone(), Leaf::Key, false),
                );
                out.push_str(&format!("<field id='{id}' style='flex-grow:1;flex-basis:0px' text='{}' label='Key' description='Rename this entry; Enter applies' action='Dispatch' />",escape(key)));
            }
        }
        out.push_str(&format!(
            "</panel><panel id='nest-value-{rid}' class='prop-value'>"
        ));
        let compound = crate::inspector::compound(db, inner(row.ty));
        if compound {
            self.collection_head(editor, field, row.ty, row.value, row.pointer, &rid, out);
        } else {
            self.leaf(editor, field, &row, &rid, out);
        }
        out.push_str("</panel>");
        // Items keep a fixed action column (Move up, Remove) so their values align;
        // struct fields keep the reset column's width.
        let width = if row.item { 44. } else { 18. };
        out.push_str(&format!("<panel id='nest-actions-{rid}' class='row' style='width:{width}px;flex-shrink:0;justify-content:end;gap:0px'>"));
        for (what, icon, label_text, calls) in row.actions {
            let button = format!("{what}-{rid}");
            self.bindings.insert(
                ara_ui::source::stable_id(&button),
                Binding::Content(
                    calls,
                    format!("{label_text} {}", sentence(&field.label).to_lowercase()),
                ),
            );
            out.push_str(&format!("<button id='{button}' class='ghost icon-button' label='{label_text}' description='{label_text}' action='Dispatch'>{}</button>",icons::icon_in(&button,icon,"icon-sm")));
        }
        out.push_str("</panel></panel>");
        if compound {
            self.items(
                editor,
                field,
                row.ty,
                row.value,
                row.pointer,
                depth + 1,
                label,
                out,
            );
        }
    }

    /// The widget for one scalar, vector, reference or enum element.
    fn leaf(&mut self, editor: &Editor, field: &Field, row: &Row<'_>, rid: &str, out: &mut String) {
        let db = editor.host.store().schema();
        let selection = &editor.state.selection;
        let (desc, value, pointer) = (row.desc, row.value, row.pointer);
        let nullable = matches!(row.ty, ValueType::Option(_));
        let ty = inner(row.ty);
        let target = &format!("{}#{pointer}", field.path);
        let set = |to: Value| set_commands(selection, field, pointer, to);
        let id = format!("nest-edit-{rid}");
        let text_box = |shell: &mut Self,
                        out: &mut String,
                        id: &str,
                        target: String,
                        leaf: Leaf,
                        text: String,
                        numeric: bool| {
            shell.bindings.insert(
                ara_ui::source::stable_id(id),
                Binding::Nested(target, leaf, nullable),
            );
            out.push_str(&format!("<field id='{id}' class='{}' style='flex-grow:1;flex-basis:0px' text='{}' label='{}' description='{}' action='Dispatch' />",if numeric{"number"}else{""},escape(&text),escape(&sentence(pointer.rsplit('/').next().unwrap_or(pointer))),escape(desc.map_or("",|d|d.doc.as_str()))));
        };
        if ty.is_number() {
            let range = desc.and_then(|d| d.range);
            if let Some((min, max)) = range.and_then(|r| Some((r.min?, r.max?))) {
                let slider = format!("nest-slider-{rid}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&slider),
                    Binding::Nested(target.to_owned(), Leaf::Number, false),
                );
                out.push_str(&format!("<slider id='{slider}' style='flex-grow:1;flex-basis:0px' min='{min}' max='{max}' step='{}' value='{}' action='Dispatch' />",desc.and_then(|d|d.editor.step).unwrap_or(0.01),value.as_f64().unwrap_or(min).clamp(min,max)));
            }
            text_box(
                self,
                out,
                &id,
                target.to_owned(),
                Leaf::Number,
                display(value),
                true,
            );
            if let Some(unit) = desc.and_then(|d| d.unit) {
                out.push_str(&format!(
                    "<text id='unit-{rid}' class='caption' text='{}' />",
                    escape(unit.symbol())
                ));
            }
            return;
        }
        if let Some(count) = ty.vector_len() {
            if *ty == ValueType::Color || desc.is_some_and(|d| d.editor.color) {
                let swatch = format!("nest-swatch-{rid}");
                out.push_str(&format!("<panel id='{swatch}' class='swatch' />"));
                self.nested_swatches.push((swatch, value.clone()));
                text_box(
                    self,
                    out,
                    &id,
                    target.to_owned(),
                    Leaf::Hex,
                    linear_hex(value),
                    false,
                );
                return;
            }
            for axis in 0..count {
                let cell = format!("{id}-{axis}");
                out.push_str(&format!("<panel id='{cell}-cell' class='vector-cell'><text id='{cell}-axis' class='axis axis-{}' text='{}' />",["x","y","z","w"][axis],["X","Y","Z","W"][axis]));
                text_box(
                    self,
                    out,
                    &cell,
                    format!("{target}/{axis}"),
                    Leaf::Number,
                    display(&value[axis]),
                    true,
                );
                out.push_str("</panel>");
            }
            return;
        }
        match ty {
            ValueType::Bool => {
                let on = value.as_bool().unwrap_or(false);
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Content(
                        set(json!(!on)),
                        format!("Set {}", sentence(&field.label).to_lowercase()),
                    ),
                );
                out.push_str(&format!(
                    "<toggle id='{id}' checked='{on}' label='{}' action='Dispatch' />",
                    escape(&sentence(pointer.rsplit('/').next().unwrap_or(pointer)))
                ));
            }
            ValueType::Asset(kind) => {
                let reference = value.as_str().unwrap_or("");
                let source = reference
                    .trim_start_matches('@')
                    .split('#')
                    .next()
                    .unwrap_or("");
                // Without its extension, like Assets; the tooltip has the reference.
                let name = Some(names::stem(names::file_name(source)))
                    .filter(|s| !s.is_empty())
                    .unwrap_or("None");
                let button = format!("asset-field-{rid}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&button),
                    Binding::AssetPicker(target.to_owned(), kind.clone()),
                );
                let preview = match self.thumbnail_slots.get(source) {
                    Some(slot) => format!(
                        "<image id='{button}-thumbnail' src='{}' style='width:16px;height:16px' />",
                        crate::asset_tools::thumbnails::image_name(*slot)
                    ),
                    None => {
                        if !source.is_empty()
                            && matches!(
                                source.rsplit('.').next(),
                                Some("material" | "graph" | "png" | "jpg" | "jpeg")
                            )
                        {
                            self.thumbnails.request(source);
                        }
                        icons::icon(&button, "package")
                    }
                };
                out.push_str(&format!("<button id='{button}' style='flex-grow:1;flex-basis:0px;min-width:0px;overflow:hidden;justify-content:start;background:var(--background);border-color:var(--input)' label='{} asset' description='{} · drop an asset here, or pick one' action='Dispatch'>{preview}<text id='asset-name-{rid}' class='{}' style='white-space:nowrap;overflow:hidden;min-width:0px;flex-shrink:1' text='{}' /></button>",escape(kind.as_deref().unwrap_or("Any")),escape(reference),if reference.is_empty(){"muted"}else{"fg"},escape(name)));
                if !reference.is_empty() {
                    let clear = format!("clear-{rid}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&clear),
                        Binding::Content(
                            set(Value::Null),
                            format!("Clear {}", sentence(&field.label).to_lowercase()),
                        ),
                    );
                    out.push_str(&format!("<button id='{clear}' class='ghost icon-button' label='Clear' description='Clear the reference' action='Dispatch'>{}</button>",icons::icon_in(&clear,"x","icon-sm")));
                }
            }
            ValueType::Named(name) => {
                let Some(Shape::Enum { variants }) = db.get(name).map(|t| &t.shape) else {
                    return;
                };
                let options: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::EnumMenu(target.to_owned(), options),
                );
                out.push_str(&format!("<button id='{id}' style='flex-grow:1;flex-basis:0px;justify-content:start' label='{}' action='Dispatch'><text id='{id}-value' class='fill' text='{}' />{}</button>",escape(name),escape(&sentence(value.as_str().unwrap_or(""))),icons::icon_in(&id,"chevron-down","icon-sm")));
            }
            ValueType::Json => {
                let text = if value.is_null() {
                    String::new()
                } else {
                    value.to_string()
                };
                text_box(self, out, &id, target.to_owned(), Leaf::Json, text, false);
            }
            // Strings and entity addresses.
            _ => {
                let nullable = nullable || *ty == ValueType::Entity;
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Nested(target.to_owned(), Leaf::Text, nullable),
                );
                out.push_str(&format!("<field id='{id}' style='flex-grow:1;flex-basis:0px' text='{}' label='{}' description='{}' action='Dispatch' />",escape(value.as_str().unwrap_or("")),escape(&sentence(pointer.rsplit('/').next().unwrap_or(pointer))),escape(desc.map_or("",|d|d.doc.as_str()))));
            }
        }
    }

    /// Paints the swatches of nested colour elements.
    pub(super) fn paint_nested_swatches(&mut self) {
        for (id, value) in &self.nested_swatches {
            let Some(i) = self.shown_node(id) else {
                continue;
            };
            let r = self.ui.nodes[i].rect;
            let clip = self.ui.nodes[i].clip.intersection(r);
            if clip.w <= 0. || clip.h <= 0. {
                continue;
            }
            let channel = |n: usize| value[n].as_f64().unwrap_or(1.).clamp(0., 1.);
            let display = &mut self.ui.display.commands;
            display.push(ara_ui::paint::Draw::PushClip { rect: clip });
            display.push(ara_ui::paint::Draw::RoundedRect {
                node: ara_ui::source::stable_id(id),
                rect: Box2 {
                    x: r.x + 2.,
                    y: r.y + 2.,
                    w: (r.w - 4.).max(0.),
                    h: (r.h - 4.).max(0.),
                },
                radius: 3.,
                color: ara_ui::style::Color(
                    (channel(0).powf(1. / 2.2) * 255.) as u8,
                    (channel(1).powf(1. / 2.2) * 255.) as u8,
                    (channel(2).powf(1. / 2.2) * 255.) as u8,
                    (value
                        .get(3)
                        .and_then(Value::as_f64)
                        .unwrap_or(1.)
                        .clamp(0., 1.)
                        * 255.) as u8,
                ),
            });
            display.push(ara_ui::paint::Draw::PopClip);
        }
    }
}

/// What an element row shows on its left.
enum Title {
    Text(String),
    /// A map entry's key, editable in place.
    Key(String),
}
/// One element row's content.
struct Row<'a> {
    title: Title,
    ty: &'a ValueType,
    desc: Option<&'a FieldDesc>,
    value: &'a Value,
    pointer: &'a str,
    /// `(id prefix, icon, label, commands)` buttons at the row's end.
    actions: Vec<(&'static str, &'static str, &'static str, Vec<Value>)>,
    /// A list item or map entry (rather than a struct field).
    item: bool,
}
/// A stable control ID for the element at `pointer` in `field`.
fn row_id(field: &Field, pointer: &str) -> String {
    format!("{}{}", field.path, pointer)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Tests assert each step at its call site.
mod tests {
    use super::*;

    #[test]
    fn drafts_apply_inside_the_whole_value() {
        let mut whole = json!({"tint": [1, 1, 1, 0.5], "cells": {"a": 1, "b": 2}, "points": []});
        // A colour keeps its alpha; empty text clears a nullable value.
        apply_draft(&mut whole, "/tint", "h", "#FF0000").unwrap();
        assert_eq!(whole["tint"], json!([1.0, 0.0, 0.0, 0.5]));
        apply_draft(&mut whole, "/cells/a", "n?", " ").unwrap();
        assert_eq!(whole["cells"]["a"], Value::Null);
        // Keys rename in place and never overwrite another entry.
        apply_draft(&mut whole, "/cells/b", "k", "c").unwrap();
        assert_eq!(whole["cells"], json!({"a": null, "c": 2}));
        assert!(apply_draft(&mut whole, "/cells/c", "k", "a").is_err());
        // One past a list's end appends; further is an error.
        set_at(&mut whole, &tokens("/points/0/x"), json!(3)).unwrap();
        assert_eq!(whole["points"], json!([{"x": 3}]));
        assert!(set_at(&mut whole, &tokens("/points/5"), json!(1)).is_err());
    }
}
