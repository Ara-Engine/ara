//! Compact first-wait and coarse progress evidence for the native Play route.
use super::*;

const LIMIT: usize = 32;
const PERIOD_SECONDS: u64 = 15;

#[derive(Default)]
pub(super) struct Samples {
    key: Option<(u32, u64)>,
    values: Vec<Value>,
}
impl Samples {
    /// Evaluate geometry only for a new phase or one coarse time bucket.
    pub(super) fn record(
        &mut self,
        age: u32,
        elapsed: Duration,
        details: impl FnOnce() -> Value,
    ) -> bool {
        let key = (age, elapsed.as_secs() / PERIOD_SECONDS);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        let sample = json!({"age":age,"elapsedMs":elapsed.as_millis(),"control":details()});
        if self.values.len() == LIMIT {
            self.values[LIMIT - 1] = sample;
        } else {
            self.values.push(sample);
        }
        true
    }
    pub(super) fn values(&self) -> &[Value] {
        &self.values
    }
    pub(super) fn last(&self) -> &Value {
        self.values.last().unwrap_or(&Value::Null)
    }
    pub(super) fn clear(&mut self) {
        self.key = None;
        self.values.clear();
    }
}

pub(super) fn control(shell: &Shell, id: &str) -> Value {
    let node = shell.shown_node(id).map(|index| &shell.ui.nodes[index]);
    let rail = shell
        .shown_node("play-options")
        .map(|index| &shell.ui.nodes[index]);
    json!({"kind":"control_reachability","selector":id,"mounted":node.is_some(),"rect":node.map(|node|node.rect),"clip":node.map(|node|node.clip),
        "matchingIds":shell.ui.nodes.iter().filter(|node|node.element.id==id).count(),"railRect":rail.map(|node|node.rect),"railClip":rail.map(|node|node.clip),"focus":shell.input.focused(),"selectOpen":shell.choice_open()})
}

pub(super) fn players(shell: &Shell, expected: usize) -> Value {
    json!({"kind":"player_readiness","selector":null,"rect":null,"clip":null,"expectedPlayers":expected,"running":shell.game_running,"fault":shell.game_fault,
        "players":shell.play_controls.players.iter().map(|(player,info)|json!({"player":player,"live":info.live,"size":info.size})).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_wait_samples_are_lazy_coarse_and_bounded() {
        let mut samples = Samples::default();
        assert!(samples.record(
            4,
            Duration::ZERO,
            || json!({"selector":"play-process-mode"})
        ));
        assert!(!samples.record(4, Duration::from_secs(14), || panic!(
            "per-frame geometry should not be sampled"
        )));
        assert!(samples.record(
            4,
            Duration::from_secs(15),
            || json!({"selector":"play-process-mode"})
        ));
        assert!(samples.record(
            5,
            Duration::from_secs(15),
            || json!({"selector":"play-client-count"})
        ));
        for age in 6..100 {
            samples.record(age, Duration::from_secs(30), || Value::Null);
        }
        assert_eq!(samples.values().len(), LIMIT);
        assert_eq!(
            samples.values()[0]["control"]["selector"],
            "play-process-mode"
        );
        assert_eq!(samples.last()["age"], 99);
        samples.clear();
        assert!(samples.record(4, Duration::ZERO, || Value::Null));
        assert_eq!(samples.values().len(), 1);
    }
}
