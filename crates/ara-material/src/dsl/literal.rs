use super::parser::Parser;
use crate::{ParameterValue, TextureDefault, TextureSource, Type, codes};
use ara_diag::Diagnostic;

impl Parser<'_> {
    /// Distinguishes literal constructors from ordinary function calls by token kind.
    pub fn literal(&mut self, declared: Type) -> Result<Option<ParameterValue>, Diagnostic> {
        let text = self.peek().to_owned();
        let parsed = if text == "[" {
            self.legacy("array literals are draft syntax; use a scalar number or vec/mat/linear/srgb constructor");
            let values = self.numbers("[", "]")?;
            if declared == Type::Bool {
                Some((Type::Bool, ParameterValue::Bool(values == [1.0])))
            } else {
                Some((declared, ParameterValue::Numeric(values)))
            }
        } else if text.starts_with('@') {
            ara_schema::grammar::parse_ref(&text).map_err(|e| self.error(e))?;
            self.take()?;
            Some((
                declared,
                ParameterValue::Texture(TextureSource::Asset(ara_schema::AssetRef(text))),
            ))
        } else if matches!(text.as_str(), "white" | "black" | "flat_normal") {
            self.take()?;
            let default = match text.as_str() {
                "white" => TextureDefault::White,
                "black" => TextureDefault::Black,
                _ => TextureDefault::FlatNormal,
            };
            Some((
                declared,
                ParameterValue::Texture(TextureSource::Default(default)),
            ))
        } else if matches!(text.as_str(), "true" | "false") {
            self.take()?;
            Some((Type::Bool, ParameterValue::Bool(text == "true")))
        } else if let Some(hex) = text.strip_prefix('#') {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(self.error("hex colors contain exactly six sRGB hex digits"));
            }
            let bits = u32::from_str_radix(hex, 16).map_err(|e| self.error(e.to_string()))?;
            self.take()?;
            Some((
                Type::ColorSrgb,
                ParameterValue::Numeric(vec![
                    f64::from((bits >> 16) & 255) / 255.0,
                    f64::from((bits >> 8) & 255) / 255.0,
                    f64::from(bits & 255) / 255.0,
                ]),
            ))
        } else if text.parse::<f64>().is_ok() {
            Some((Type::Float, ParameterValue::Numeric(vec![self.number()?])))
        } else {
            let ty = match text.as_str() {
                "vec2" => Some(Type::Vec2),
                "vec3" => Some(Type::Vec3),
                "vec4" => Some(Type::Vec4),
                "mat3" => Some(Type::Mat3),
                "mat4" => Some(Type::Mat4),
                "linear" => Some(Type::ColorLinear),
                "srgb" => Some(Type::ColorSrgb),
                _ => None,
            };
            let numeric = self
                .tokens
                .get(self.cursor + 2)
                .is_some_and(|t| t.text.parse::<f64>().is_ok());
            if let Some(ty) = ty.filter(|_| numeric) {
                self.take()?;
                Some((ty, ParameterValue::Numeric(self.numbers("(", ")")?)))
            } else {
                None
            }
        };
        let Some((actual, value)) = parsed else {
            return Ok(None);
        };
        if declared.is_texture() && !matches!(value, ParameterValue::Texture(_)) {
            self.legacy("texture slot numbers are not authored values; use @path#id or a built-in default (the suggested fix uses a fallback)");
            return Ok(Some(ParameterValue::zero(declared)));
        }
        if actual != declared {
            self.issues.push(Diagnostic::error(codes::TYPE,format!("literal has type {actual}, expected {declared}; the fix supplies an explicit typed default")));
            return Ok(Some(ParameterValue::zero(declared)));
        }
        Ok(Some(value))
    }
}
