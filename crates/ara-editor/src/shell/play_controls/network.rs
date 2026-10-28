use super::*;
pub(super) const PROFILES: &[(&str, &str)] = &[
    ("clean", "Clean · 0 ms"),
    ("lan", "LAN · 5 ms"),
    ("internet", "Internet · 50 ms"),
    ("mobile", "Mobile · 100 ms"),
    ("stress", "Stress · 200 ms, 2% loss"),
];
fn condition(name: &str) -> Value {
    let (latency, jitter, loss) = match name {
        "lan" => (5, 0, 0),
        "internet" => (50, 10, 0),
        "mobile" => (100, 30, 100),
        "stress" => (200, 50, 200),
        _ => return Value::Null,
    };
    json!({"latencyMs":latency,"jitterMs":jitter,"loss":loss,"duplication":0,"reordering":0,"bandwidth":0})
}
/// Client-order shaping for the existing play.start contract (basis-point loss).
pub(crate) fn conditions(settings: &BTreeMap<String, Value>, clients: u32) -> Vec<Value> {
    (1..=clients.min(16))
        .map(|player| {
            condition(
                settings
                    .get("play.networkProfiles")
                    .and_then(|map| map.get(player.to_string()))
                    .and_then(Value::as_str)
                    .unwrap_or("clean"),
            )
        })
        .collect()
}
impl Shell {
    pub(crate) fn set_play_network_profile(
        &mut self,
        editor: &mut Editor,
        player: usize,
        value: Value,
    ) -> ara_ui::Result<()> {
        if !(1..=16).contains(&player)
            || !value
                .as_str()
                .is_some_and(|name| PROFILES.iter().any(|profile| profile.0 == name))
        {
            return Err("Choose a supported client network profile".into());
        }
        let mut profiles = editor
            .state
            .settings
            .get("play.networkProfiles")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        profiles.insert(player.to_string(), value);
        editor.action(
            "editor.settings.set",
            json!({"key":"play.networkProfiles","value":profiles}),
        )?;
        self.rebuild(editor)
    }
    pub(super) fn play_network_markup(&mut self, editor: &Editor, out: &mut String) {
        let clients = editor
            .state
            .settings
            .get("play.clients")
            .and_then(Value::as_f64)
            .unwrap_or(1.)
            .clamp(0., 16.) as usize;
        out.push_str("<text id='play-network-hint' class='caption' text='Network conditions apply on the next Play start.' />");
        for player in 1..=clients {
            out.push_str(&format!("<panel id='play-network-row-{player}' class='row' style='gap:8px'><text id='play-network-label-{player}' class='caption' text='Client {player}' />"));
            self.choice_button(
                &format!("play-network-{player}"),
                &format!("Client {player} network"),
                choices::Select {
                    destination: choices::Destination::PlayNetwork(player),
                    options: PROFILES
                        .iter()
                        .map(|(value, label)| (json!(value), (*label).into()))
                        .collect(),
                    current: editor
                        .state
                        .settings
                        .get("play.networkProfiles")
                        .and_then(|map| map.get(player.to_string()))
                        .cloned()
                        .unwrap_or_else(|| json!("clean")),
                },
                out,
            );
            out.push_str("</panel>");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_order_and_basis_points_reach_existing_start_contract() {
        let settings = BTreeMap::from([(
            "play.networkProfiles".into(),
            json!({"1":"stress","3":"mobile"}),
        )]);
        let shaped = conditions(&settings, 3);
        assert_eq!(shaped.len(), 3);
        assert_eq!(shaped[0]["latencyMs"], 200);
        assert_eq!(shaped[0]["loss"], 200);
        assert!(shaped[1].is_null());
        assert_eq!(shaped[2]["loss"], 100);
        assert_eq!(conditions(&settings, 100).len(), 16);
        assert!(conditions(&settings, 0).is_empty());
    }
}
