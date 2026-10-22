# ara-merge

Semantic three-way merge of documents over their flat record maps: the engine
behind `ara merge` and the git merge driver. Depends on ara-doc, ara-schema,
ara-json, ara-diag.

## Public API

- `merge(&base, &ours, &theirs, &db) -> Result<Merged, Diagnostic>`
  (`ARA1702` if ours and theirs are different documents).
- `Merged { doc, conflicts }`; `doc.conflicts` already holds the `$conflicts`
  entries, so `ara_doc::write` emits a complete file.
- `Conflict { record, path, base, ours, theirs, reason }`, `to_json()`,
  `to_diagnostic(file)` (`ARA1701`).

## Rules

- Per record, then per object key, recursively. One side changed → that side wins;
  both changed equally → fine; both changed differently → conflict.
- Arrays and scalars are atomic (vectors, `removed` lists and keyed lists conflict
  as a whole).
- Delete vs modify keeps the modified record and reports a conflict; delete vs
  unchanged deletes.
- After merging, the hierarchy is checked: dangling parents, parent loops and
  root-count problems become conflicts on `parent`.
- On conflict the output keeps ours (or the surviving side) and lists the conflict
  in `$conflicts`, keeping the file strict JSON. Reading it reports `ARA1209` until
  the entries are removed.
- Sibling `order` keys minted on both branches may collide; ties sort by ID, so
  that is not a conflict.

## Git

```
# .gitattributes
*.scene  merge=ara
*.prefab merge=ara
# .git/config
[merge "ara"]
    name = Ara semantic merge
    driver = ara merge %O %A %B
```

`ara merge` writes the result to `%A` and exits 1 when there are conflicts, which
git treats as a conflicted merge.

## Testing

`cargo test -p ara-merge`: disjoint changes (snapshot), field conflicts (snapshot +
re-read flags them), delete/modify, both-add, hierarchy loops and dangling parents,
per-key override maps.
