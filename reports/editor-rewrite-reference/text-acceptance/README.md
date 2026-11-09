# Accepted-text history boundary

Prepared D-only against 9b700c513. Apply accepted-text.patch serially; no repository source or index has been changed. Eight files; six focused tests prepared but NOT compiled or run. Formatting and git apply --check passed. Actual application in a plain scratch tree reproduced exact LF source bytes with core.autocrlf=false. No GPU work or native process was launched.

## Confirmed failure

See ../collection-diagnosis/evidence.json. Native 82eaead1 collection cases 1 and 2 passed. Mixed list edit committed tx_kzdtamr1 (base120 -> result121), one set op on e_collectionnative1 MeshRenderer.emissiveMaterials, only index100: item100 -> item 100 changed through native input. Secondary collection was untouched as required by its shorter length. After native CtrlZ at phase9, authority remains rev121 with exactly the committed model digest. The same focused widget ID6283418570522422699 instead changed display text back to item100. There is no inverse transaction. This is missing accepted-text history ownership, not kernel Undo corruption or an evidence timing race.

## Product fix

TextInput::accept_current() clears draft Undo/Redo only after actual host acceptance, preserving value/caret/selection. Active IME is preserved and returns false. can_undo/can_redo expose local capacity. Ui::accept_text(index) delegates without changing focus or emitting another action. Router yields an empty focused Undo/Redo to its host's existing command policy, while real draft history and active composition retain local ownership.

Do not infer acceptance from Ui::reconcile or a model-value echo: those paths intentionally preserve edits/caret/history because hosts echo drafts on every keystroke. Do not blur the native journey merely to make it pass.

Editor commit_fields now wraps its existing implementation. It records focused stable ID and authority revision, invokes the complete existing commit path, and acknowledges only after success plus an actual authoring revision advance. Failure/validation errors and asynchronous-only job acknowledgements preserve local history. This central seam covers focused ordinary/nested/mixed Inspector fields, names, graph/material/VFX/animation drafts, picker text and project-policy draft branches that produce accepted typed ops. Native numeric grips already lower their committed dispatch to commit_fields. Material DSL apply_resource_text is the synchronous CtrlEnter/button bypass and has an explicit acknowledgement after typed apply succeeds.

Scope limits: this patch does not clear unrelated unfocused fields or infer async rename/source-job success from an initial preparing response. Async filename controls are retired by their existing completed-job UI; if any retained async text editor survives success, its owning final-result callback must call Ui::accept_text then. User-only navigation/settings history is not authoring Host history and is deliberately not acknowledged merely because a UI draft was echoed. Hub/game hosts can use the same explicit kit API when their model accepts a field.

## Required Root checks

- ara-ui-text --test acceptance (2 tests): acceptance preserves caret/selection; active IME remains unchanged.
- ara-ui --test text_acceptance (1 test): same-value draft echo keeps undo; explicit acceptance yields empty Undo/Redo with focus retained.
- ara-editor --lib text_acceptance_tests (3 tests): real mixed Inspector Enter commit -> focused CtrlZ typed Undo -> CtrlY typed Redo, exact revisions/records; uncommitted Undo/Redo stays local; rejected numeric validation retains draft Undo and model state.
- Full touched-crate tests, strict all-target Clippy, editor --lib, fmt; then rebuild matching Windows editor/player and rerun the unchanged raw-SDL collections route. No native acceptance is established by this candidate.

The unchanged collection driver remains intentionally strict: one CtrlZ and one CtrlY with the field still focused must traverse the accepted typed history. No driver-only focus workaround, registry shortcut, new diagnostic code, public C# API, binary ABI, registry or reflection type was added.
