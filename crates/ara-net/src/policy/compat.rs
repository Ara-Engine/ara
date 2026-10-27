//! Compatibility covers transmitted values, never inspector metadata or local modules.
use super::*;
use serde_json::json;

impl PolicyRegistry {
    pub(super) fn compatibility_hash(&self) -> Result<[u8; 32]> {
        let mut contract = Vec::new();
        for (&id, field) in &self.fields {
            if field.policy.audience == Audience::None {
                continue;
            }
            let mut versions = BTreeMap::new();
            let mut owner = self
                .schema
                .get(&field.component)
                .ok_or_else(|| NetError("missing component schema".into()))?;
            versions.insert(owner.name.clone(), owner.version);
            // Flattened struct fields still depend on their containing value versions.
            for segment in &field.path {
                let desc = owner
                    .field(segment)
                    .ok_or_else(|| NetError("missing compiled field schema".into()))?;
                if let ValueType::Named(name) = &desc.ty {
                    owner = self
                        .schema
                        .get(name)
                        .ok_or_else(|| NetError(format!("unknown schema {name}")))?;
                    versions.insert(owner.name.clone(), owner.version);
                }
            }
            let mut values = BTreeMap::new();
            self.compatibility_value(&field.ty, 0, &mut values)?;
            contract.push(json!({
                "id":id,"component":field.component,"path":field.path,
                "type":field.ty.to_string(),"policy":field.policy,"unit":field.unit,
                "versions":versions,"values":values
            }));
        }
        Ok(ara_sim::StateHash::of_bytes(
            ara_json::to_canonical_string(&ara_json::sort_keys(&Value::Array(contract))).as_bytes(),
        )
        .0)
    }

    fn compatibility_value(
        &self,
        ty: &ValueType,
        depth: usize,
        out: &mut BTreeMap<String, Value>,
    ) -> Result<()> {
        if depth > 16 {
            return Err(NetError("recursive wire compatibility schema".into()));
        }
        match ty {
            ValueType::List(t) | ValueType::Map(t) | ValueType::Keyed(t) | ValueType::Option(t) => {
                self.compatibility_value(t, depth + 1, out)?;
            }
            ValueType::Named(name) => {
                let desc = self
                    .schema
                    .get(name)
                    .ok_or_else(|| NetError(format!("unknown schema {name}")))?;
                let fields: BTreeMap<_, _> = desc
                    .fields()
                    .iter()
                    .map(|field| (field.name.as_str(), field.ty.to_string()))
                    .collect();
                let mut variants: Vec<_> =
                    desc.variants().iter().map(|v| v.name.as_str()).collect();
                variants.sort_unstable();
                out.insert(name.clone(), json!({"version":desc.version,"kind":desc.kind_name(),"fields":fields,"variants":variants}));
                for field in desc.fields() {
                    self.compatibility_value(&field.ty, depth + 1, out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
