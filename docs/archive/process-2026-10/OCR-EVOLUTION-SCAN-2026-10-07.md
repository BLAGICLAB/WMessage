
─── tests-audit/audit_evolution_layering.py:147-150 ───
[bug · critical] strip_comments_and_strings confuses Rust lifetimes with char literals. A bare 'a /
'static / '_ in &'static str or fn resolve<'a>(...) sets in_char=True and stays in that mode until
the next single quote, so all intervening source — including the body of any subsequent string
literal — is blanked out and newlines are replaced by spaces. Any forbidden pattern (e.g.
evolution::policy, tauri::) sitting after such a lifetime in strategy.rs / context / data files is
silently missed, and the line numbers reported for later matches are wrong. The preprocessor needs
to recognise that ' outside of b'...' is a lifetime and should be treated as ordinary source.



─── tests-audit/audit_evolution_layering.py:159-159 ───
[bug · high] strip_cfg_test_modules only recognises a #[cfg(test)] attribute when it is followed by
a newline before mod. The very common single-line form #[cfg(test)] mod tests { is not matched, so
the body of that test module is still scanned and any forbidden token inside it (e.g. a use
rand::... in a test) is reported as a real layering violation — i.e. false positives that the whole
point of the stripper is to avoid. Use a pattern that allows arbitrary horizontal whitespace, e.g.
#\[cfg\(test\)\][ \t]*\bmod\s+(\w+)\s*\{ (or re.MULTILINE + [^\n]*\bmod).



─── tests-audit/audit_evolution_layering.py:62-67 ───
[bug · medium] Several forbidden patterns are not anchored with \b on the left, so they will fire on
any identifier that ends in the same suffix — the regex engine does substring matching.
evolution::panel, evolution::policy, tauri:: and chrono::Utc::now would all match e.g.
pre_evolution::panel, my_tauri::App, or even a renamed re-export such as super_chrono::Utc::now. Add
a leading \b (or anchor to 'use ' / 'crate::' / 'super::') so only the genuine paths are caught.



─── tests-audit/audit_evolution_layering.py:223-230 ───
[test · medium] The --selftest fixtures (SELFTEST_PASS and SELFTEST_FAIL_SAMPLES) only contain ASCII
identifier keywords and never exercise the two most fragile paths in the preprocessor — Rust
lifetime/char collision and inline #[cfg(test)] mod foo {. As a result the regression guard reports
green even when those bugs ship, so the layering contract it claims to enforce is not actually
verified by selftest. Add a positive case containing &'static str / <'a> / inline #[cfg(test)] mod
to make sure the preprocessor handles them, plus a negative case showing a violation sitting after a
lifetime is still detected.



─── src-tauri/src/evolution/candidate/mod.rs:52-64 ───
[bug · high] Check-then-act race between `read_all` and `append`: if two callers invoke
`write_proposals` concurrently (e.g. parallel `post_consolidation` runs, or overlapping Tauri
command dispatches), both can snapshot the same `existing_ids` set, both can find a given
`proposal_id` missing, and both will append the same entry. The jsonl append model means duplicates
silently accumulate instead of corrupting prior lines, so the failure mode is exactly what the dedup
is supposed to prevent — and downstream notification consumers in R6 A will then surface the same
change twice. Consider serializing writes with an `fs2`/`fd-lock` file lock, or rewriting the batch
as a single atomic temp-file + rename that rebuilds the dedup baseline in the same critical section.

### Code
```rust
    // read_all Err（首行结构级损坏）必须传播——dedup 基线不可得时盲写会重复追加
    let existing = entry::read_all(&path)?;
    let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
    let mut written: Vec<ProposalEntry> = Vec::new();
    let now_ms = chrono::Utc::now().timestamp_millis();
    for p in proposals {
        if existing_ids.contains(&p.proposal_id) {
            continue;
        }
        let entry = derive::from_proposal(p, now_ms);
        entry::append(&path, &entry)?;
        written.push(entry);
    }
```

-     // read_all Err（首行结构级损坏）必须传播——dedup 基线不可得时盲写会重复追加
+     // 在文件锁内重建 dedup 基线 + 追加，杜绝并发重复写入
+     let _guard = crate::db::paths::lock_exclusive(&path)?;
      let existing = entry::read_all(&path)?;
      let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
-     let mut written: Vec<ProposalEntry> = Vec::new();
+     let mut written: Vec<ProposalEntry> = Vec::with_capacity(proposals.len());
      let now_ms = chrono::Utc::now().timestamp_millis();
      for p in proposals {
          if existing_ids.contains(&p.proposal_id) {
              continue;
          }
          let entry = derive::from_proposal(p, now_ms);
          entry::append(&path, &entry)?;
          written.push(entry);
      }


─── src-tauri/src/evolution/candidate/mod.rs:46-49 ───
[maintainability · medium] Public API returns `Result<Vec<ProposalEntry>, String>`, which collapses
the underlying `entry::read_all` / `entry::append` errors into opaque strings at this boundary.
Callers like `evolution::post_consolidation` (and any R6 shadow hook that bubbles failures up)
cannot distinguish a corrupted-line parse error from an I/O error from a permission error, and
cannot attach actionable context (path, line offset, syscall). Prefer a typed error (e.g.
`thiserror::Error` enum with `Io`, `Decode { line, source }`, `Encode` variants) and `?`-propagate
it; only convert to a string at the outermost Tauri command boundary where the framework requires
it.

### Code
```rust
pub fn write_proposals<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    proposals: &[crate::evolution::proposal::EvolutionProposal],
) -> Result<Vec<ProposalEntry>, String>
```

  pub fn write_proposals<R: tauri::Runtime>(
      app: &tauri::AppHandle<R>,
      proposals: &[crate::evolution::proposal::EvolutionProposal],
- ) -> Result<Vec<ProposalEntry>, String> {
+ ) -> Result<Vec<ProposalEntry>, crate::evolution::candidate::CandidateError> {


─── src-tauri/src/evolution/candidate/mod.rs:53-64 ───
[performance · medium] `entry::read_all` reads the entire jsonl into memory and `entry::append`
performs a synchronous filesystem write; both are invoked here on the calling thread. Because this
function takes `&AppHandle<R>` and is documented as being called from
`evolution::post_consolidation`, it is very likely invoked from a Tauri async command handler or a
`tokio::spawn`ed task — running synchronous file I/O on either path will block the runtime worker
for the duration of the read/write and stall unrelated commands. Either make `write_proposals`
`async` and push the I/O onto `tokio::task::spawn_blocking`, or have it delegate to an existing
blocking-pool helper, so cancellation and other commands remain responsive.

### Code
```rust
    let existing = entry::read_all(&path)?;
    let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
    let mut written: Vec<ProposalEntry> = Vec::new();
    let now_ms = chrono::Utc::now().timestamp_millis();
    for p in proposals {
        if existing_ids.contains(&p.proposal_id) {
            continue;
        }
        let entry = derive::from_proposal(p, now_ms);
        entry::append(&path, &entry)?;
        written.push(entry);
    }
```

+     let path = path.clone();
+     let proposals = proposals.to_vec();
+     let (existing_ids, mut written) = tokio::task::spawn_blocking(move || -> Result<_, CandidateError> {
-     let existing = entry::read_all(&path)?;
+         let existing = entry::read_all(&path)?;
-     let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
+         let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
-     let mut written: Vec<ProposalEntry> = Vec::new();
+         let mut written: Vec<ProposalEntry> = Vec::with_capacity(proposals.len());
-     let now_ms = chrono::Utc::now().timestamp_millis();
+         let now_ms = chrono::Utc::now().timestamp_millis();
-     for p in proposals {
+         for p in &proposals {
-         if existing_ids.contains(&p.proposal_id) {
+             if existing_ids.contains(&p.proposal_id) {
-             continue;
+                 continue;
-         }
+             }
-         let entry = derive::from_proposal(p, now_ms);
+             let entry = derive::from_proposal(p, now_ms);
-         entry::append(&path, &entry)?;
+             entry::append(&path, &entry)?;
-         written.push(entry);
+             written.push(entry);
-     }
+         }
+         Ok((existing_ids, written))
+     })
+     .await
+     .map_err(|e| CandidateError::Join(e.to_string()))??;


─── src-tauri/src/evolution/candidate/mod.rs:54-55 ───
[performance · medium] `existing_ids` materializes a freshly-cloned `String` for every prior
proposal_id in the file on every batch write, and the function reads the whole file before writing
anything. Under the documented 14-day TTL the file will grow into thousands of entries, so this is
O(n) memory + O(n) time per call even when only a handful of new proposals are being added. A
streaming dedup (read line-by-line, short-circuit on first match per id, or keep a bounded
LRU/sidecar bloom of recent ids) would keep the working set proportional to the incoming batch
rather than the full history. Also `Vec::new()` is allocated for `written` while the upper bound is
known.

### Code
```rust
    let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
    let mut written: Vec<ProposalEntry> = Vec::new();
```

-     let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
-     let mut written: Vec<ProposalEntry> = Vec::new();
+     let mut written: Vec<ProposalEntry> = Vec::with_capacity(proposals.len());
+     // 流式扫描 jsonl：每行只比对传入 batch 的 id，不全量收集
+     let incoming_ids: HashSet<&str> = proposals.iter().map(|p| p.proposal_id.as_str()).collect();
+     let mut seen: HashSet<String> = HashSet::with_capacity(incoming_ids.len());
+     entry::scan_ids_in_file(&path, |id| {
+         if incoming_ids.contains(id.as_str()) {
+             seen.insert(id.to_string());
+         }
+     })?;
+     let now_ms = chrono::Utc::now().timestamp_millis();
+     for p in proposals {
+         if seen.contains(&p.proposal_id) {
+             continue;
+         }
+         let entry = derive::from_proposal(p, now_ms);
+         entry::append(&path, &entry)?;
+         written.push(entry);
+     }


─── src-tauri/src/evolution/candidate/derive.rs:48-50 ───
[bug · medium] TTL clock is anchored to the pool-insertion time (`now_ms`) rather than the
proposal's `created_at_ms`. A proposal created hours/days before being pooled effectively gets a
full TTL on top of its pre-pool age, extending its effective lifetime well past the documented
window. If the spec intends "TTL since proposal creation", pass `p.created_at_ms` (and use `now_ms`
only as a fallback or for jitter). If the spec truly means "TTL since pooling", capture that intent
in the function name (e.g. `from_proposal_at`) so the contract is unambiguous at call sites.



─── src-tauri/src/evolution/candidate/derive.rs:34-34 ───
[bug · high] `from_proposal` forwards a caller-supplied `now_ms: i64` straight into
`compute_expires_at(now_ms)` with no sanity check (e.g. `now_ms >= 0`, `now_ms <= i64::MAX -
TTL_MS`). A malformed/untrusted value near `i64::MAX` will overflow in the TTL addition and silently
produce a wrapped `expires_at_ms`, breaking TTL semantics. Either validate the input here (return a
`Result`, or clamp/panic with a clear message) or document and enforce the precondition at the
boundary.



─── src-tauri/src/evolution/candidate/derive.rs:35-47 ───
[performance · low] `from_proposal` takes `&EvolutionProposal` but then `.clone()`s five fields
(`proposal_id`, `target`, `suggestion_text`, `related_refs`, `summary`) just to satisfy the owned
`ProposalEntry` contract. If callers don't need the proposal afterward (typical for a derivation
step), change the signature to `from_proposal(p: EvolutionProposal, now_ms: i64) -> ProposalEntry`
(or `p: &EvolutionProposal` returning `Result` with a borrowed DTO) and let `mem::take`/move
semantics eliminate all five allocations.



─── src-tauri/src/evolution/candidate/derive.rs:62-70 ───
[test · low] `mk_proposal` always builds a `ProposalTarget::MemoryPolicy`, even when the test
parameter is `ProposalCategory::ToolSchemaHint` (in `from_proposal_starts_pooled`) or `PromptHint`.
This lets the layer-mapping tests pass while the target/category pair is semantically nonsensical,
masking real bugs in downstream code that expects the two to be consistent. Build a `ProposalTarget`
that matches the requested category (or take it as a parameter) so the test data reflects a valid
proposal.



─── src-tauri/src/evolution/candidate/derive.rs:23-31 ───
[maintainability · low] `derive_change_id` / `derive_mem_key` blindly prepend `"chg-"` / `"evo:"` to
`proposal_id` with no escaping, length check, or character validation, and the contract "must match
apply.rs" is only enforced by a single equality assertion in
`derive_change_id_and_mem_key_format_locked`. If `proposal_id` ever contains separators (`/`, `:`,
whitespace) from a future external source, the resulting keys can collide or be ambiguous
downstream. Either add an input validation helper (e.g. assert ASCII alphanumeric + `-/_`) or
document the invariant near the helper and add a property-style test covering unusual IDs.



─── src-tauri/src/evolution/candidate/conflict.rs:43-50 ───
[maintainability · low] Redundant proposal_id check: `is_conflict(e, new)` already returns false
when `e.proposal_id == new.proposal_id`, so the trailing `&& e.proposal_id != new.proposal_id`
clause is dead. If the id-exclusion rule in `is_conflict` is ever relaxed or changed, this duplicate
must be kept in sync; drop it and rely on the single source of truth in `is_conflict`.

  pub fn find_conflict<'a>(
      entries: &'a [ProposalEntry],
      new: &ProposalEntry,
  ) -> Option<&'a ProposalEntry> {
-     entries
-         .iter()
-         .find(|e| is_conflict(e, new) && e.proposal_id != new.proposal_id)
+     entries.iter().find(|e| is_conflict(e, new))
  }


─── src-tauri/src/evolution/candidate/conflict.rs:64-70 ───
[bug · medium] No guard for same-proposal_id / same-reference inputs: `resolve_conflict`
unconditionally forwards to `DefaultEvolutionPolicy::resolve`. If a caller ever passes two
references to the same `ProposalEntry` (or two entries that share a `proposal_id`), the documented
contract "higher impact wins, ties go to older" says nothing about this case and the delegate's
behavior is unspecified here, so the returned `(winner, loser)` tuple could contain the same entry
on both sides or violate the "impact higher wins / older wins" invariant. Either document the
precondition (require distinct proposal_ids) or short-circuit when `a.proposal_id == b.proposal_id`.

  pub fn resolve_conflict<'a>(
      a: &'a ProposalEntry,
      b: &'a ProposalEntry,
  ) -> (&'a ProposalEntry, &'a ProposalEntry) {
+     debug_assert!(
+         a.proposal_id != b.proposal_id,
+         "resolve_conflict requires distinct proposal_ids"
+     );
      let r = crate::evolution::strategy::DefaultEvolutionPolicy.resolve(a, b);
      (r.winner, r.loser)
  }


─── src-tauri/src/evolution/candidate/conflict.rs:64-70 ───
[maintainability · medium] Body exceeds the documented "one-line delegation" invariant: the
file-level comment at line ~17 explicitly states that any function body longer than a single line of
delegation must fail CI (不变式 10). `resolve_conflict` has a two-statement body (a `let` plus a tuple
reconstruction). Consider collapsing the `ResolveResult` conversion into the return so the body is a
single delegation expression and the invariant is mechanically satisfied.

  pub fn resolve_conflict<'a>(
      a: &'a ProposalEntry,
      b: &'a ProposalEntry,
  ) -> (&'a ProposalEntry, &'a ProposalEntry) {
      let r = crate::evolution::strategy::DefaultEvolutionPolicy.resolve(a, b);
      (r.winner, r.loser)
  }


─── src-tauri/src/evolution/candidate/conflict.rs:83-86 ───
[maintainability · medium] Test module calls deprecated APIs without `#[allow(deprecated)]`: every
test here invokes `layer_priority`, `impact_ord`, `resolve_conflict`, and
`sort_entries_cross_layer`, all of which carry `#[deprecated(...)]`. Under `RUSTFLAGS="-D warnings"`
(commonly paired with the "CI fail" invariant cited above) these will break the build. Add
`#![allow(deprecated)]` at the top of the `mod tests` block (or convert the tests to call
`strategy::DefaultEvolutionPolicy` directly, which is the migration goal).

  #[cfg(test)]
+ #[allow(deprecated)] // migration phase: tests intentionally exercise the still-live deprecated wrappers
  mod tests {
      use super::*;
      use crate::evolution::proposal::{ProposalOrigin, ProposalTarget};


─── src-tauri/src/evolution/candidate/conflict.rs:35-37 ───
[bug · medium] Conflict identity depends on `ProposalTarget::tag()` being a lossless key.
`is_conflict` treats any two entries as conflicting whenever their target discriminants share a tag,
regardless of the payload. For `ProposalTarget::MemoryPolicy { policy }`, two entries with different
`policy` strings would share the same discriminant tag and would be treated as conflicting —
silently dropping the more recent/specific one. Either confirm `tag()` includes the discriminating
payload (e.g. `MemoryPolicy/{policy}`) or add a unit test that exercises two `MemoryPolicy` entries
with distinct `policy` strings to lock the contract.



─── src-tauri/src/evolution/candidate/mapping.rs:37-40 ───
[maintainability · high] The wildcard arm `_ => ApprovalSource::Pending` silently assigns Pending to
`ChangeStatus::Expired` and to any future variants (e.g. `Approved`/`Applied`). This produces
internally inconsistent records (the doc comment explicitly acknowledges `Expired → Pending`, and
`to_change_record_expired_entry` then locks that behavior in). Prefer an exhaustive match and either
add an `ApprovalSource::Expired` variant (or map Expired to `SystemRejected`), so future
`ChangeStatus` variants surface at compile time instead of being silently mis-tagged as Pending.



─── src-tauri/src/evolution/candidate/mapping.rs:29-29 ───
[maintainability · medium] `hard_constraint_compliance: bool` is a bare boolean that drives status
derivation inside what is otherwise a pure data-mapping function. This conflates caller-side policy
with the mapping and lets a caller combine any entry status with any compliance flag without a
type-level guard. Consider taking a pre-resolved `ChangeStatus` (caller applies the compliance
override) or introducing a small typed wrapper such as `HardConstraintVerdict::{Compliant,
NonCompliant}` so the policy branch is explicit at the call site and can't be silently inverted.



─── src-tauri/src/evolution/candidate/mapping.rs:43-43 ───
[maintainability · medium] `parent_id: None` is annotated `// 根` but `ProposalEntry` carries no
signal that the entry is a root, so the function silently produces a disconnected `ChangeRecord` if
it is ever invoked with a child entry. Either propagate a `parent_id` (or `parent:
Option<&ProposalEntry>`) from the entry, or add a `debug_assert!` / explicit invariant so misuse
fails loudly instead of dropping parent linkage downstream.



─── src-tauri/src/evolution/candidate/entry.rs:84-85 ───
[bug · high] Cross-process write race: the doc-comment itself admits "无 flock" — when `observe_run`
and the main process both write to evolution-proposals.jsonl, POSIX O_APPEND is per-process, so the
single-write atomicity guarantee does not hold across processes and JSONL lines can be
torn/overlapped. A real `flock`/`fcntl` lock-file (or `fs2`/`fd-lock`) must be acquired before the
write to make this safe under the documented cross-process scenario.



─── src-tauri/src/evolution/candidate/entry.rs:85-85 ───
[maintainability · medium] Caller-must-hold-lock contract is unenforced at the API boundary:
`append(path, entry)` does not statically or dynamically require the caller to hold
`evolution::lock_evolution_store`. A new caller who forgets the lock will silently produce torn
lines with no compile-time hint. Consider a newtype wrapper (e.g. `pub struct
LockedEvolutionStore<'a>(..)`) whose `append` method is the only way to obtain the file handle, so
the unsafe-without-lock usage is unrepresentable.



─── src-tauri/src/evolution/candidate/entry.rs:0-0 ───
[bug · medium] All I/O and serde errors are flattened to `Result<_, String>` via `format!`, dropping
the original `std::io::Error` / `serde_json::Error` source chain and making programmatic matching by
callers impossible. Define a typed error (e.g. `pub enum ProposalIoError { Io(std::io::Error),
Serde(serde_json::Error) }` with `#[from]` impls) or at minimum `Box<dyn std::error::Error + Send +
Sync>`. This also applies to `read_all` which propagates the upstream `read_jsonl` error type as-is
— unifying on one project-wide error type would help.



─── src-tauri/src/evolution/candidate/entry.rs:54-68 ───
[maintainability · low] `ProposalStatus::as_str` hand-rolls the same `"snake_case"` mapping that
`#[serde(rename_all = "snake_case")]` already produces on the wire. If a variant is added/renamed,
the two can silently disagree and `status_strings_locked` will only pin one half. Either derive
`as_str` from the serde name (e.g. via a serde-generated helper) or drop it if its only consumer is
tests — the `Debug`/`Display` impls on the enum are sufficient for diagnostics.



─── src-tauri/src/evolution/candidate/entry.rs:21-35 ───
[bug · medium] `change_id` and `mem_key` are documented as derivations of `proposal_id` (`"chg-" +
proposal_id` / `"evo:" + proposal_id`) but are stored as independent `String` fields with no
validation or invariant check. A caller that constructs a `ProposalEntry` manually with mismatched
values produces silently corrupt data — downstream code that joins on these keys will mis-route. Add
an `fn derive_from(proposal_id: &str) -> (String, String)` helper and/or a constructor like
`ProposalEntry::from_proposal(...)` that always derives them, so the invariant is centralized in one
place.



─── src-tauri/src/evolution/candidate/ttl.rs:62-69 ───
[security · high] Saturating to `i64::MAX` for adversarial `created_at_ms` defeats the eviction loop
instead of preserving the audit trail. `is_expired` checks `expires_at_ms <= now_ms`, so once
`expires_at_ms` saturates to `i64::MAX`, no realistic wall-clock `now_ms` will ever trip it —
meaning the entry stays in `ProposalStatus::Pooled` forever, `mark_expired` never flips it, and
`evict_expired`'s `retain` predicate stays false. The "preserve audit trail" rationale only
justifies saturation for already-terminal states (`Promoted` / `Rejected`); for `Pooled`
(pre-review) entries it instead yields indefinite retention that `evict_expired` cannot recover
from. Recommend either (a) clamping `created_at_ms` to a sane upper bound (e.g. `now_ms + TTL_MS`)
at jsonl ingestion rather than at expiry computation, or (b) having `evict_expired` also drop
entries whose `expires_at_ms` exceeds any plausible horizon, so adversarial jsonl input cannot grow
the pool without bound.

  /// 计算 expires_at_ms（候选条目创建/续期用）。
  ///
  /// 饱和加法：created_at_ms 来自 jsonl（不可信输入），近 i64::MAX 的
  /// 腐败/对抗值不会让结果 wrap 成负数——溢出时饱和到 i64::MAX =
  /// **永不过期**（条目留存保审计轨迹），而非静默立刻过期被淘汰。
+ ///
+ /// 注意：饱和仅对已终态（Promoted/Rejected）条目安全；Pooled 条目
+ /// 因此进入「永不蒸发」状态，evict_expired 无法回收。建议在 jsonl
+ /// 反序列化层对 created_at_ms 做上界校验（如 ≤ now_ms + TTL_MS）。
  pub fn compute_expires_at(created_at_ms: i64) -> i64 {
      created_at_ms.saturating_add(TTL_MS)
  }


─── src-tauri/src/evolution/candidate/ttl.rs:34-37 ───
[maintainability · medium] The "now_ms must share its source with created_at_ms/expires_at_ms"
contract is documented but unenforced — a caller passing a monotonic-clock millisecond, or a `now_ms
< created_at_ms` after NTP rollback, will silently produce wrong expiration decisions with no panic,
log, or `debug_assert!`. The module-level comment explicitly acknowledges NTP回拨/时钟回跳 as a known
fragility, yet the comparison site has zero defensive checks. For a 14-day TTL the wall-clock
rollback risk is small; consider at minimum a `debug_assert!(now_ms >= 0)` and `debug_assert!(now_ms
>= entry.created_at_ms.saturating_sub(TTL_MS))` so misuse is caught in tests, or convert `now_ms` to
a newtype (`WallClockMs`) so the type system refuses cross-source comparisons.

  /// 该 entry 是否已过期（expires_at_ms <= now_ms）
  pub fn is_expired(entry: &ProposalEntry, now_ms: i64) -> bool {
+     debug_assert!(now_ms >= 0, "now_ms must be a wall-clock Unix ms");
+     debug_assert!(
+         entry.expires_at_ms >= 0,
+         "expires_at_ms must be non-negative (got {})",
+         entry.expires_at_ms
+     );
      entry.expires_at_ms <= now_ms
  }


─── src-tauri/src/evolution/candidate/ttl.rs:51-60 ───
[maintainability · medium] `evict_expired` is a footgun the API does not defend against. It silently
drops expired `Pooled` entries that have not yet been flipped to `Expired` by `mark_expired`,
breaking the audit-trail guarantee the rest of the module promises. Worse, the function returns only
a `usize` count, forcing callers that want to snapshot deleted rows (per the doc's own
recommendation) to diff before/after themselves — an easy mistake to skip. Recommend either (a)
returning `Vec<ProposalEntry>` of removed entries so the caller can persist them with no extra work,
or (b) panicking in debug / returning an error in release when called with `now_ms` that would drop
`Pooled` rows still in `Pooled` status, to surface ordering bugs at the boundary instead of silently
corrupting history.

- /// 硬淘汰：移除「过期且 status ∈ {Pooled, Expired}」的条目（返回淘汰数）。
+ /// 硬淘汰：移除「过期且 status ∈ {Pooled, Expired}」的条目（返回被淘汰条目供审计快照）。
  /// Promoted / Rejected 超期不删——它们承载审计/回滚轨迹，硬删会丢历史。
- pub fn evict_expired(entries: &mut Vec<ProposalEntry>, now_ms: i64) -> usize {
-     let before = entries.len();
-     entries.retain(|e| {
-         !(is_expired(e, now_ms)
-             && matches!(e.status, ProposalStatus::Pooled | ProposalStatus::Expired))
-     });
-     before - entries.len()
+ pub fn evict_expired(
+     entries: &mut Vec<ProposalEntry>,
+     now_ms: i64,
+ ) -> Vec<ProposalEntry> {
+     let mut evicted = Vec::new();
+     let mut i = 0;
+     while i < entries.len() {
+         let drop = is_expired(&entries[i], now_ms)
+             && matches!(
+                 entries[i].status,
+                 ProposalStatus::Pooled | ProposalStatus::Expired
+             );
+         if drop {
+             evicted.push(entries.remove(i));
+         } else {
+             i += 1;
+         }
+     }
+     evicted
  }


─── src-tauri/src/evolution/activation.rs:273-292 ───
[other · high] Concurrency hazards in save_state: (a) `SAVE_STATE_LOCK` is a synchronous Mutex held
across blocking `std::fs::read_to_string` + `serde_json::from_str` + `atomic_write` I/O — if
save_state is ever invoked from a Tauri async command/handler this will block the runtime thread;
consider parking the RMW on a `spawn_blocking` task or using a process-wide writer queue. (b)
Poisoning recovery via `e.into_inner()` silently continues even when a prior holder panicked
mid-RMW, so the config file may already be in an inconsistent state by the time save_state runs — at
minimum log a loud warning and ideally abort/retry rather than silently overwriting. (c) The lock
only guards writers that go through save_state; the in-source comment acknowledges bot/config
writers outside this lock can still interleave, producing a classic read‑modify‑write TOCTOU where
the comment says "last writer wins = silent loss of transitions". Given §12.7 mandates "保留其它字段", the
RMW should be replaced with an actual atomic compare-and-swap (e.g. advisory lock + reread-and-retry
on mismatch, or rewrite via `tempfile` + rename on a versioned slot) instead of trusting an
in-process mutex that nothing else honors.

- /// save_state 的 RMW 锁（bot-config.json 读→改→写全程）。
+ /// save_state 的 RMW 锁（仅在 spawn_blocking 临界区内持有，见下方）。
  static SAVE_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
  
- /// 保存当前状态到 bot-config.json 的 evolution.activation_state 字段
- ///
- /// 老板 22:02 spec v4.1 §12.7：“运行时切态必须写回，不能只手动改文件”。
- /// 写后原文件其它字段保留。
- /// RMW 全程持 SAVE_STATE_LOCK（并发 save_state 最后写者胜 = 静默丢转移）；
- /// 最终写走 atomic_write（tmp+rename）——std::fs::write 直写崩溃会截断整个
- /// bot-config.json（§12.7 要求保留其它字段）。
- /// 残余：bot/config 其它写者不在此锁内（跨写者统一锁 = follow-up）。
- ///
- /// **实验态（B4-5 登记）**：当前无生产调用（切态 Tauri 命令等 §6 候选生成器，
- /// 见 OBSERVATION_STATUS §2「有意未做」）；S0 观察态设计等真数据后接线，
- /// 接口按 spec 冻结。
  pub fn save_state(state: ActivationState, config_path: &Path) -> Result<(), String> {
+     // 在 spawn_blocking 中持锁，绝不在 async runtime 线程上阻塞持有。
      let _g = SAVE_STATE_LOCK.lock().unwrap_or_else(|e| {
-         eprintln!("[mutex_poisoned] evolution::activation::SAVE_STATE_LOCK: {e:?}");
-         e.into_inner()
+         // 中毒代表先前写者中途 panic，bot-config.json 状态不可信；
+         // 至少拒绝继续覆盖，避免把不一致状态静默落盘。
+         return Err(format!(
+             "[mutex_poisoned] evolution::activation::SAVE_STATE_LOCK: {e:?}; \
+              refusing to overwrite bot-config.json after a poisoned prior write"
+         ));
      });
+ 
+     // RMW：读 → 修改 → 写 必须在同一持锁段完成，并对外部并发写者做冲突检测。
+     // 简易方案：写前重新读 + diff，若 evolution.activation_state 与期望不一致，
+     // 拒绝覆盖并要求调用者重试（避免最后写者胜丢转移）。
+     let raw = std::fs::read_to_string(config_path)
+         .map_err(|e| format!("读 {config_path:?} 失败：{e}"))?;
+     let mut v: serde_json::Value =
+         serde_json::from_str(&raw).map_err(|e| format!("解析 {config_path:?} 失败：{e}"))?;


─── src-tauri/src/evolution/activation.rs:94-120 ───
[maintainability · medium] Type-design issue: `mode` is a free-form `String` with magic-string
compares (`mode == "calibrating"`, `mode == "active"`). A typo like `"Active"`, `" observe"`,
`"active "` silently disables activation AND makes `is_calibrating()` return false, producing a
config that is neither calibrated nor active. The doc-comment lists four valid values (`calibrating
| observe | suggest | active`) but no runtime check enforces them, and `should_auto_trigger` only
returns true for exactly `"active"` + both thresholds — the documented `"observe"`/`"suggest"` modes
silently no-op with no diagnostic. Model this as an enum (with `#[serde(rename_all =
"snake_case")]`) so invalid states are unrepresentable and parse failures surface at load time
rather than at behavior time.

+ #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
+ #[serde(rename_all = "snake_case")]
+ pub enum ActivationMode {
+     #[default]
+     Calibrating,
+     Observe,
+     Suggest,
+     Active,
+ }
+ 
  #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
  #[serde(default, rename_all = "snake_case")]
  pub struct ActivationConfig {
      /// calibrating | observe | suggest | active
      /// calibrating: 阈值未填，不自动触发 S1（骨架阶段）
-     pub mode: String,
+     pub mode: ActivationMode,
      /// 同偏好重复次数（主判据），null = 未校准
      pub min_occurrences: Option<u32>,
      /// 全局兜底 proposal 数，null = 未校准
      pub min_proposals: Option<u32>,
  }
  
  impl ActivationConfig {
      pub fn is_calibrating(&self) -> bool {
-         self.mode == "calibrating"
+         matches!(self.mode, ActivationMode::Calibrating)
      }
  
      /// 是否应该自动触发 S1→S1（占位阶段总返 false）
      pub fn should_auto_trigger(&self, _current_count: u64) -> bool {
          // 骨架阶段：calibrating 下永不自动触发（老板 21:38 拍板）
          // 其他模式：必须有阈值才能触发（防误 block）
-         if self.mode == "calibrating" {
+         if self.is_calibrating() {
              return false;
          }
-         self.mode == "active" && self.min_occurrences.is_some() && self.min_proposals.is_some()
+         matches!(self.mode, ActivationMode::Active)
+             && self.min_occurrences.is_some()
+             && self.min_proposals.is_some()
      }
  }


─── src-tauri/src/evolution/activation.rs:124-155 ───
[maintainability · medium] Silent error swallowing in loaders: `load_config_from_file` /
`load_state_from_file` treat parse failure, IO error (non-NotFound), schema mismatch, and unknown
enum string identically — every one collapses to `default()` with only an `eprintln!` to stderr.
Callers and tests cannot distinguish "intentionally unconfigured" from "config exists but corrupt /
typo'd". Combined with `mode` being a free-form `String` (see other comment), a manual edit like
`mode: "Active"` is silently ignored with the same observable effect as a missing file — no
diagnostic, no metric, no test failure. Consider returning `Result<ActivationConfig, ConfigError>`
(or at least a richer enum like `LoadOutcome::Default | LoadOutcome::Loaded(_) |
LoadOutcome::Invalid { reason }`) so the caller can surface misconfiguration in the UI and tests can
assert on it. The current "lenient by design" comment justifies the behavior but does not justify
the loss of observability.

- /// 从 bot-config.json 读 activation 块（lenient）。
- /// 缺文件/缺块 = 合法默认模式静默；**存在但坏**（IO 错 / parse 失败 / schema 不匹配）留痕。
- pub fn load_config_from_file(path: &Path) -> ActivationConfig {
+ /// 从 bot-config.json 读 activation 块。
+ /// 缺文件/缺块 = 合法默认模式静默；
+ /// 存在但坏（IO 错 / parse 失败 / schema 不匹配）= 返 Invalid 让上层 UI 提示。
+ pub fn load_config_from_file(path: &Path) -> LoadOutcome<ActivationConfig> {
      let raw = match std::fs::read_to_string(path) {
          Ok(r) => r,
-         Err(e) => {
-             if e.kind() != std::io::ErrorKind::NotFound {
-                 eprintln!("[evolution_activation] 读 {path:?} 失败（{e}），activation 用默认值");
-             }
-             return ActivationConfig::default();
+         Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
+             return LoadOutcome::Default; // 缺文件 = 未配置，合法静默
          }
+         Err(e) => return LoadOutcome::Invalid { reason: format!("读 {path:?} 失败：{e}") },
      };
      let v = match serde_json::from_str::<serde_json::Value>(&raw) {
          Ok(v) => v,
-         Err(e) => {
-             eprintln!("[evolution_activation] {path:?} JSON 解析失败（{e}），activation 用默认值");
-             return ActivationConfig::default();
-         }
+         Err(e) => return LoadOutcome::Invalid { reason: format!("{path:?} JSON 解析失败：{e}") },
      };
      let Some(activation) = v.get("evolution").and_then(|e| e.get("activation")) else {
-         return ActivationConfig::default(); // 缺块 = 未配置，合法静默
+         return LoadOutcome::Default; // 缺块 = 未配置，合法静默
      };
-     match serde_json::from_value(activation.clone()) {
-         Ok(c) => c,
-         Err(e) => {
-             eprintln!(
-                 "[evolution_activation] {path:?} activation 块 schema 不匹配（{e}），用默认值"
-             );
-             ActivationConfig::default()
-         }
-     }
+     serde_json::from_value(activation.clone())
+         .map(LoadOutcome::Loaded)
+         .unwrap_or_else(|e| LoadOutcome::Invalid {
+             reason: format!("{path:?} activation 块 schema 不匹配：{e}"),
+         })
  }


─── src-tauri/src/evolution/activation.rs:203-218 ───
[maintainability · medium] API misuse hazard: `shadow_route` accepts `ActivationState` but
immediately discards it with `let _ = state;` — all three states route identically for reversible
proposals. The header comment promises "4 态 audit 在 wrapper 区分", but the function itself performs no
per-state branching, so callers wiring it into per-state audit logic will assume state-sensitive
behavior that does not exist. Tests only assert the S0/S1/S2 equivalence rather than a per-state
contract, so any future refactor that "accidentally" differentiates one state will silently change
the audit story. Either drop the parameter until real per-state routing is wired, or add a
`#[deprecated]` / `#[doc(hidden)]` marker, or branch on `state` to make the API honest. At minimum,
remove the `let _ = state;` dead-binding and have the function explicitly state in its doc that
`state` is currently reserved and ignored.

  /// shadow 路由决策（pure function，便于测试）
  ///
  /// 骨架阶段行为：
  /// - 不可逆 → Skip
  /// - 可逆 + 任何状态 → WriteShadow（4 态 audit 在 wrapper 区分）
  ///
  /// **实验态（B4-5 登记）**：当前无生产调用（shadow 走全量观察，不经此路由），
  /// S0 观察态设计等真数据（OBSERVATION_STATUS §2/§3）后接线；接口按 spec 冻结。
+ ///
+ /// **警告**：当前 `state` 参数被刻意忽略——S0/S1/S2 在本函数中行为完全相同。
+ /// 调用方若依赖按状态路由，请同时走 4 态 audit 的 wrapper，否则会得到错误预期。
  pub fn shadow_route(state: ActivationState, p: &EvolutionProposal) -> RouteDecision {
      if !is_reversible(p) {
          return RouteDecision::Skip;
      }
-     let _ = state; // 骨架阶段：S0/S1/S2 行为相同（都写 shadow jsonl）
-                    // 未来 B 校准：S2 同时写主记忆
+     // 显式吞 state，避免未来无意修改时悄悄引入 per-state 行为。
+     debug_assert!(matches!(
+         state,
+         ActivationState::S0Observe | ActivationState::S1Suggest | ActivationState::S2Active
+     ));
      RouteDecision::WriteShadow
  }


─── src-tauri/src/evolution/activation.rs:255-269 ───
[maintainability · medium] Dead-code / contract drift hazard in `evaluate_s2`: the function
unconditionally returns `S2Decision::Allow { reason: "占位：真 policy 未填，默认 allow".into() }` regardless
of input, so `S2Decision::Block { reason: String }` is unreachable from any current caller. There is
no test that exercises the `Block` arm of `as_str()`/`reason()` (only `Allow` is asserted). When
real policy wiring lands, a regression that forgets to construct `Block { reason }` (e.g.
matches-and-returns-Allow-on-default) will pass existing tests silently. Also, the placeholder
reason string contains the literal `"占位"` — downstream consumers that surface this reason to the
user / log to audit will leak "占位：真 policy 未填" into the audit trail, falsely implying a real policy
decision was made. Either (a) make the placeholder reason obviously non-user-facing (e.g. include a
marker the audit layer can detect and replace with `Expire` / `NoPolicyApplied`), or (b) split the
placeholder into a separate `S2Decision::Placeholder` variant that the audit layer translates to
`AuditDecision::NoPolicyApplied`, so the contract — "S2 with no policy must never look like a real
Allow" — is enforced by the type system.

  /// S2 evaluate 占位（老板 22:10 拍板，选 A 方案）
  ///
  /// 重要：骨架阶段永远返 Allow。is_reversible 已在路由前过（防御纵深），
  /// 不可逆 proposal 根本不到这里。所以 evaluate_s2 不会看到「高风险」，Block 不可达。
  ///
  /// Block 变体不是死代码——是为 B 校准阶段真 policy 准备的位置。
  /// 当真 policy 加载后（填了 min_occurrences / min_proposals / 高风险规则），
  /// evaluate_s2 会查表匹配，返回 Block。
  ///
  /// 当前占位原因：记录「未生效」+「真 policy 未填」，防止误判「S2 已生效」。
  pub fn evaluate_s2(_p: &EvolutionProposal) -> S2Decision {
-     S2Decision::Allow {
-         reason: "占位：真 policy 未填，默认 allow".into(),
-     }
+     // 用专用变体让审计层明确区分「占位默认」与「真 policy 命中 Allow」，
+     // 避免 reason 字符串泄漏「占位」字样到 audit 轨道。
+     S2Decision::Placeholder
  }


─── src-tauri/src/evolution/apply.rs:81-92 ───
[performance · high] `apply_one` does `store::load_all(conn)?` and scans the full Vec twice
(tags-any-position then content) for every call. Inside the batch loop in
`apply_from_consolidation`, this is O(proposals × memory_rows) per batch with two full passes each —
quadratic in batch × store size. `store::find_by_key_tag(&conn, &key)` already exists (used by tests
+ `rollback_applied`) and is the targeted/indexed lookup; the slow "any tag position" scan should be
a *fallback* only when the fast indexed lookup misses, not the primary path. Two
`drop(all)`-equivalent full loads also bloat RSS when embeddings are loaded into the rows.

      let key = evolution_key(&p.proposal_id);
-     let all = store::load_all(conn)?;
-     if all.iter().any(|m| m.tags.iter().any(|t| t == &key)) {
+     // Fast path: indexed key-tag lookup (rollback/tests already rely on this).
+     if store::find_by_key_tag(conn, &key)?.is_some() {
          return Ok(ApplyOutcome::AlreadyPresent);
      }
-     if all
-         .iter()
-         .any(|m| m.kind == "lesson" && m.content == p.suggestion.text)
-     {
+     // Slow path: tags may have been reordered by merge; only fall back to a
+     // full scan when the indexed lookup misses.
+     let all = store::load_all(conn)?;
+     let dup = all.iter().any(|m| {
+         m.tags.iter().any(|t| t == &key)
+             || (m.kind == "lesson" && m.content == p.suggestion.text)
+     });
+     drop(all);
+     if dup {
          return Ok(ApplyOutcome::AlreadyPresent);
      }
-     drop(all);


─── src-tauri/src/evolution/apply.rs:374-389 ───
[other · medium] Fire-and-forget shadow spawn drops its JoinHandle. Per project rules, spawned tasks
whose failures/cancellation/shutdown must be observed should keep or `abort` the handle —
`tauri::async_runtime::spawn(async move { ... shadow_apply_for_batch_with_app(...).await; })`
discards any panic, JoinError, or runtime shutdown signal silently. The main spawn above *does*
`await` its handle so it is OK, but the shadow sibling task here is not observable.



─── src-tauri/src/evolution/apply.rs:182-189 ───
[performance · medium] The comment claims shadow cloning is "廉价（reference count bump）" but
`proposals: Vec<EvolutionProposal>` is owned (each `EvolutionProposal` holds `String` proposal_id,
summary, text, structured_patch Option, related_refs Vec<...>), so `proposals.clone()` performs a
real deep allocation proportional to the *sum of every owned String* in every proposal — exactly the
case where the `Arc<Wry>`-clone metaphor is wrong. For large batches (many proposals or long
`suggestion.text`), this is wasted memory and CPU; either wrap in `Arc<[EvolutionProposal]>` once at
the call boundary or feed shadow via a borrowed slice + a small lookup table.



─── src-tauri/src/evolution/apply.rs:273-281 ───
[other · medium] Inside `apply_from_consolidation`, the per-proposal `Applied` arm re-enters
`lock_evolution_store()` *twice* in immediate succession (once for `append_applied_record`, once for
the change-record block below), and each `append_applied_record` call performs
`create_dir_all(parent)` + `OpenOptions::open()` + `writeln!` *while holding the
EVOLUTION_STORE_LOCK*. For a batch of N proposals that means 2N mutex acquisitions plus 2N
`mkdir`+`open` syscalls *all serialized under the same lock*, blocking every other evolution-store
writer (shadow, panel). Hoist file open + parent mkdir *outside* the per-proposal loop (open once,
mkdir once), drop the lock during the write, and either merge the two EVOLUTION_STORE_LOCK scopes or
use a single guard for both appends.



─── src-tauri/src/evolution/apply.rs:236-247 ───
[bug · medium] `embed_text` returning `None` is logged via `evolution.embed_failed` audit but the
proposal is *still applied* with `emb.as_deref() = None` (the `Option` is threaded straight into
`apply_one` and then `store::insert_item`). The in-source comment correctly notes "无向量的 lesson
永不可语义召回" — the lesson is committed to the memory store but is permanently invisible to vector-recall
queries, wasting storage and skewing the lesson corpus. Either skip `apply_one` (treat as a
transient `Applied=false` outcome / per-proposal `apply_skipped_embed` counter) or hard-fail the
proposal with `ApplyOutcome::EmbedFailed`. The audit also only reports `failed: <count>`; the
per-proposal identity is lost, which makes post-mortem diagnosis impossible.



─── src-tauri/src/evolution/apply.rs:98-100 ───
[maintainability · low] `apply_one` hardcodes
`crate::evolution::strategy::DefaultEvolutionPolicy.importance(p.impact)` while the rest of
`apply_from_consolidation` already builds an `EvalContext { apply_policy:
read_apply_policy_at(&kill_cfg_path), kill_switch, shadow_enabled, now_ms }` and threads that
through the surrounding code path. Two independent policy sources now coexist: the gate /
kill-switch / apply_policy from config, but importance from the default concrete type. If
`apply_policy` ever differs from `DefaultEvolutionPolicy` (which is exactly what
`EvalContext.apply_policy` exists to support), the lesson's importance will silently diverge from
what the gate passed. Thread the resolved `&dyn EvolutionPolicy` (or at least the importance value)
into `apply_one`'s signature, or compute importance in `apply_from_consolidation` and pass it in.



─── src-tauri/src/evolution/apply.rs:140-150 ───
[performance · low] `append_applied_record` calls `std::fs::create_dir_all(parent)` on every
invocation. The parent directory is invariant for the whole batch (it's `{data_dir}/` or
`{data_dir}/sub/`); checking it once before the batch loop would avoid a syscall per proposal. Same
function also lacks an `fsync`/`flush` — the file handle from `OpenOptions` is dropped right after
`writeln!` without `f.sync_all()`, so a crash between the apply commit and the OS write-back can
lose the audit row even though `applied.jsonl` is described as "审计与人工回滚的依据". Combined with the
per-call `open` noted above, the entire I/O pattern of this ledger is heavier and weaker than it
needs to be.



─── src-tauri/src/evolution/observe/mod.rs:1-12 ───
[documentation · medium] Stale/incomplete module-structure documentation: the doc comment lists only
`metrics` and `synthetic` in the "模块结构" enumeration, but the file declares four public submodules
(`metrics`, `shadow`, `stop`, `synthetic`). The overview should either enumerate all four (and
describe each) or drop the bullet list, otherwise readers will get a misleading picture of the
layer's surface and the read-only/no-LLM/no-new-schema invariant will appear to apply only to two of
them.



─── src-tauri/src/evolution/observe/mod.rs:14-18 ───
[maintainability · low] Inconsistent re-export naming convention: `compute` is aliased to
`compute_metrics` and `generate` to `generate_synthetic`, but `check_stop_condition` is re-exported
under its original name. The asymmetry looks like API drift rather than an intentional scheme —
either always prefix with the module name for disambiguation, or never do it, and document the rule
at the crate root so the public surface stays predictable.



─── src-tauri/src/evolution/observe/mod.rs:1-7 ───
[maintainability · low] Documented architectural invariant is not enforced at this boundary: the doc
comment claims the layer "不调 LLM；不写新数据库表；只读 evolution-proposals/changes/applied.jsonl", but `mod.rs`
itself does nothing to enforce this — the guarantee depends entirely on each submodule's discipline.
Consider adding a short audit note (or a CI/lint check) so future additions to `shadow`/`stop`/etc.
cannot silently violate the stated contract.



─── src-tauri/src/evolution/change/status.rs:42-45 ───
[bug · high] Skip-canary hard-constraint is not enforced by the API. `can_transition` and
`transition` both unconditionally accept `Approved → Active` on pure topology, while the module doc
admits a missing external policy gate = hard-constraint ② bypass. Any caller wiring `transition`
directly into the state setter without layering its own kill-switch / skip-canary guard silently
breaks the sandbox-skip protection. Consider encoding skip-canary as a separate variant (e.g.
require an explicit `Approved → Active` request that carries a policy token) or rejecting `Approved
→ Active` here and pushing the edge into a distinct helper that policy code must call by name.



─── src-tauri/src/evolution/change/status.rs:61-65 ───
[maintainability · medium] Errors are flattened to `Result<(), String>`, collapsing three
semantically distinct outcomes (same-state, terminal source, illegal edge) into opaque strings.
Callers / audit / metrics code cannot pattern-match and must resort to `e.contains("终态")` style
parsing, which is fragile if messages ever change. Return a typed error (e.g. `enum TransitionError
{ SameState, FromTerminal, IllegalEdge(ChangeStatus, ChangeStatus) }`) implementing `Display` /
`std::error::Error` so observability and tests can assert on variants instead of substrings.



─── src-tauri/src/evolution/change/status.rs:52-57 ───
[maintainability · medium] TTL-to-`Expired` is encoded as a hardcoded literal list of non-terminal
states, which is fragile to future `ChangeStatus` additions (e.g. a hypothetical `Paused` state
would silently lose its TTL exit). The accompanying `any_non_terminal_to_expired_ok` test only
iterates the same literal list, so it would not catch the omission either. Prefer deriving this from
the topology itself — e.g. `(_, Expired) if !from.is_terminal()` plus an explicit `Active` exit arm
— so every non-terminal state gets `→ Expired` automatically and the test can iterate
`ChangeStatus::all_non_terminal()` (or equivalent) instead of a hand-maintained array.



─── src-tauri/src/evolution/observe/metrics.rs:60-64 ───
[bug · high] Window filter boundary mismatch between comment and code. The inline comment documents
`created_at_ms >= window_start_ms && < now_ms` (strict upper bound), but the filter actually uses
`<= now_ms`. A proposal stamped exactly at `now_ms` will be counted contrary to the documented
semantics. Either align the filter to the documented half-open interval (`< now_ms`) or update the
comment, and make sure the test suite pins down the chosen convention.

-     // 窗口内候选数（created_at_ms >= window_start_ms && < now_ms）
+     // 窗口内候选数（created_at_ms >= window_start_ms && < now_ms，半开区间）
      let proposals_in_window: usize = proposals
          .iter()
-         .filter(|p| p.created_at_ms >= window_start_ms && p.created_at_ms <= now_ms)
+         .filter(|p| p.created_at_ms >= window_start_ms && p.created_at_ms < now_ms)
          .count();


─── src-tauri/src/evolution/observe/metrics.rs:68-84 ───
[bug · high] The observation window is honored by `candidate_generation_rate` only; `promoted_count`
/ `approval_rate`, `rolled_back_count` / `rollback_rate`, and `pollution_survival_days` are computed
over the entire history of `proposals` / `changes`. A round of R6 observation should report
window-scoped values consistently — otherwise a long-running system will have these four ratios
drift away from each other and `observation_window_days` becomes meaningless metadata. Filter
`proposals` / `changes` / `applied` by `window_start_ms`/`now_ms` (e.g. introduce helper
`in_window(p)` and apply it before counting promoted / rolled-back / active) or document explicitly
that these three ratios are intentionally all-time and add a corresponding `*_in_window` variant.

-     // 通过：proposal.status == Promoted
+     // 通过：窗口内 proposal.status == Promoted
      let promoted_count = proposals
          .iter()
-         .filter(|p| p.status == ProposalStatus::Promoted)
+         .filter(|p| p.created_at_ms >= window_start_ms
+             && p.created_at_ms <= now_ms
+             && p.status == ProposalStatus::Promoted)
          .count();
  
-     // 回滚：ChangeRecord.status == RolledBack
+     // 回滚：窗口内 ChangeRecord.status == RolledBack
      let rolled_back_count = changes
          .iter()
-         .filter(|c| c.status == ChangeStatus::RolledBack)
+         .filter(|c| c.created_at_ms >= window_start_ms
+             && c.created_at_ms <= now_ms
+             && c.status == ChangeStatus::RolledBack)
          .count();
  
-     // 当前 active 的 ChangeRecord 数（仍生效）
+     // 当前 active 的 ChangeRecord 数（仍生效；按窗口）
      let active_changes = changes
          .iter()
-         .filter(|c| c.status == ChangeStatus::Active)
+         .filter(|c| c.created_at_ms >= window_start_ms
+             && c.created_at_ms <= now_ms
+             && c.status == ChangeStatus::Active)
          .count();


─── src-tauri/src/evolution/observe/metrics.rs:66-66 ───
[bug · high] `window_days` silently rewrites a zero or negative window into `1.0`. If `now_ms <=
window_start_ms` (caller bug, clock skew, or non-monotonic `now_ms`), this masks the problem and
reports a fake 1-day rate. Either propagate the error (return `Result<ObserveMetrics,
MetricsError>`) or at least guard with `debug_assert!` / log a warning, and never silently coerce
invalid input into a metric.

-     let window_days = ((now_ms - window_start_ms) as f64 / 86_400_000.0).max(1.0);
+     debug_assert!(
+         now_ms >= window_start_ms,
+         "compute: now_ms ({now_ms}) < window_start_ms ({window_start_ms})"
+     );
+     let window_days = ((now_ms - window_start_ms).max(0) as f64 / 86_400_000.0).max(1.0);


─── src-tauri/src/evolution/observe/metrics.rs:96-107 ───
[bug · medium] Silent data drops in `pollution_survival_days`. Two distinct populations are excluded
from the average without any signal: (1) `Active` ChangeRecords whose `mem_key` has no matching
`AppliedRecord` — these silently vanish from the numerator AND the denominator, biasing the average
toward lessons that *do* have an applied record; (2) applied records with `applied_at_ms > now_ms`
(clock skew / future-dated entries) are dropped via `if days >= 0.0`. At minimum, log a warning /
counter when these drops occur; better, return them as fields on `ObserveMetrics` (e.g.
`survival_missing_applied`, `survival_future_dated`) so callers can decide.

      let mut survival_total_days = 0.0;
      let mut survival_n = 0u64;
+     let mut survival_missing_applied = 0u64;
+     let mut survival_future_dated = 0u64;
      for change in changes.iter().filter(|c| c.status == ChangeStatus::Active) {
-         // 查 applied.jsonl：找 mem_key 对应的 applied_at_ms
-         if let Some(app) = applied_by_key.get(change.mem_key.as_str()) {
+         match applied_by_key.get(change.mem_key.as_str()) {
+             None => survival_missing_applied += 1,
+             Some(app) => {
-             let days = (now_ms - app.applied_at_ms) as f64 / 86_400_000.0;
+                 let days = (now_ms - app.applied_at_ms) as f64 / 86_400_000.0;
-             if days >= 0.0 {
+                 if days >= 0.0 {
-                 survival_total_days += days;
+                     survival_total_days += days;
-                 survival_n += 1;
+                     survival_n += 1;
+                 } else {
+                     survival_future_dated += 1;
+                 }
              }
          }
      }


─── src-tauri/src/evolution/observe/metrics.rs:134-134 ───
[maintainability · low] `active_lessons` is named "lessons" but is populated from the count of
`ChangeRecord::Active`. Throughout the file the data model distinguishes proposals / changes /
applied records / lessons; readers will reasonably assume `active_lessons` is "live entries in the
lesson store" and may compare it against `pollution_survival_days`'s denominator, which is actually
a different population (active changes that have an applied record match). Either rename to
`active_changes` to match the local variable and source-of-truth, or document the distinction and
the relationship between `active_lessons` and the survival average.

-         active_lessons: active_changes,
+         // Number of ChangeRecord entries currently Active (lessons that have not been rolled back).
+         active_changes,


─── src-tauri/src/evolution/change/derive.rs:103-114 ───
[bug · critical] TOCTOU race in `unique_change_id_for`: three concurrent writers (panel toggle /
shadow / apply CR) each pass their own snapshot of `rows` into this function and write the resulting
id straight to jsonl. Two writers that race here will both see the same candidate as "not taken" and
both pick it, producing the exact duplicate-id rows the comment says cause "second rollback
permanent deadlock". The check-then-write has no atomicity. Either serialize appends behind a single
writer (file lock, channel, mutex around the jsonl file) or perform the suffix allocation under the
same lock that appends the row, so the snapshot used to pick the id is the same snapshot committed
to disk.

  pub fn unique_change_id_for(rows: &[ChangeRecord], proposal_id: &str) -> String {
      let base = derive_change_id(proposal_id);
-     let taken = |id: &str| rows.iter().any(|c| c.change_id == id);
-     if !taken(&base) {
+     let taken: std::collections::HashSet<&str> =
+         rows.iter().map(|c| c.change_id.as_str()).collect();
+     if !taken.contains(base.as_str()) {
          return base;
      }
      for n in 2..=9999 {
          let candidate = format!("{base}-{n}");
-         if !taken(&candidate) {
+         if !taken.contains(candidate.as_str()) {
              return candidate;
+         }
-         }
+     }
+     // ...timestamp fallback
-     }
+ }


─── src-tauri/src/evolution/change/derive.rs:69-71 ───
[maintainability · high] `from_proposal` still routes through the `#[deprecated]`
`passes_auto_apply_gate` even though its own deprecation note tells callers to switch to
`EvolutionPolicy::gate` directly. This emits a deprecation warning on every build of the production
constructor and defeats the point of marking the wrapper deprecated. Call
`matches!(DefaultEvolutionPolicy.gate(p), GateDecision::Approved)` here and drop the wrapper once
nothing else references it.

  pub fn from_proposal(p: &EvolutionProposal, now_ms: i64) -> ChangeRecord {
-     let compliance = passes_auto_apply_gate(p);
+     let compliance = matches!(
+         crate::evolution::strategy::DefaultEvolutionPolicy.gate(p),
+         crate::evolution::strategy::GateDecision::Approved
+     );
      let (status, approval_source) = if compliance {


─── src-tauri/src/evolution/change/derive.rs:0-0 ───
[security · medium] `proposal_id` is formatted directly into `change_id` and `mem_key` with no
sanitization. If it ever contains newlines (`\n`/`\r`), tabs, NULs, embedded quotes, or is empty,
the resulting strings can corrupt jsonl row parsing (a stray `\n` will split a row in two), pollute
logs, or collide as a mem key. Add a validation/sanitization step at the boundary (reject empty,
reject control chars, or normalize to a safe charset) before letting it reach these formatters, or
encode the suffix portion (e.g., base64 / a hash) so the output is guaranteed well-formed.

  pub fn derive_change_id(proposal_id: &str) -> String {
+     debug_assert!(!proposal_id.is_empty() && proposal_id.chars().all(|c| c.is_ascii_graphic()));
      format!("chg-{}", proposal_id)
  }
  
  pub fn derive_mem_key(proposal_id: &str) -> String {
+     debug_assert!(!proposal_id.is_empty() && proposal_id.chars().all(|c| c.is_ascii_graphic()));
      format!("evo:{}", proposal_id)
  }


─── src-tauri/src/evolution/change/derive.rs:105-114 ───
[performance · low] `unique_change_id_for` does a fresh `rows.iter().any(...)` linear scan inside
the `2..=9999` loop, giving O(n*k) work where k is the number of colliding suffixes. For large row
counts or repeated suffix collisions this is wasteful and harder to reason about than a single set
membership check. Build a `HashSet<&str>` of taken change_ids once before the loop (or just track
the highest existing `-{n}` suffix for `base` and return `base-(max+1)`), which is O(n) total and
removes the per-candidate scan.

-     let taken = |id: &str| rows.iter().any(|c| c.change_id == id);
-     if !taken(&base) {
+     let taken: std::collections::HashSet<&str> =
+         rows.iter().map(|c| c.change_id.as_str()).collect();
+     if !taken.contains(base.as_str()) {
          return base;
      }
      for n in 2..=9999 {
          let candidate = format!("{base}-{n}");
-         if !taken(&candidate) {
+         if !taken.contains(candidate.as_str()) {
              return candidate;
          }
      }


─── src-tauri/src/evolution/change/record.rs:183-192 ───
[bug · critical] Concurrent append/read race: `append` does `create_dir_all` → `open(O_APPEND)` →
`write_all` as separate syscalls with no exclusive file lock, and `read_all` (via
`crate::evolution::read_jsonl`) reads the same file with no synchronization. Two concurrent `append`
calls can have their `write_all` syscalls interleaved by the kernel, producing two half-lines
concatenated into one corrupted JSONL record; a concurrent `read_all` during `append` can observe a
partial line and surface it as a "corrupt middle line". The doc string defers to a lock contract in
`candidate::entry::append`, but that contract is neither imported nor enforced here, so any caller
that bypasses the candidate layer (or any future caller in this module) will see corruption. Wrap
the file with `fs2::FileExt::lock_exclusive` (or `flock(2)`) for the duration of write+sync, or
funnel all writes through a serialized writer task.

- pub fn append(path: &std::path::Path, record: &ChangeRecord) -> Result<(), String> {
-     use std::io::Write;
-     if let Some(parent) = path.parent() {
-         std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
-     }
+ // Pseudo-patch: hold an exclusive flock for the lifetime of write+sync.
+ use fs2::FileExt;
-     let mut f = std::fs::OpenOptions::new()
+ let mut f = std::fs::OpenOptions::new()
-         .create(true)
-         .append(true)
-         .open(path)
-         .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
+     .create(true).append(true).open(path)?;
+ f.lock_exclusive().map_err(|e| format!("flock 失败：{e}"))?;
+ // ... write_all(line.as_bytes()) ...
+ f.sync_all().map_err(|e| format!("fsync 失败：{e}"))?;
+ // lock released on drop.


─── src-tauri/src/evolution/change/record.rs:195-197 ───
[bug · high] No `sync_all()`/`fsync` after `write_all`. The module header markets
`evolution-changes.jsonl` as 持久化, but the bytes here stay in the OS page cache until the file is
closed or the kernel decides to flush; on power loss, panic, or `kill -9` the just-appended record
can be lost, leaving the changelog silently truncated and breaking any downstream reconstruction
that assumes a complete append log (e.g. `find_children`, lineage traversal in R3). Add an explicit
`f.sync_all()` (or `sync_data()`) before `Ok(())`, and pair it with the exclusive lock from the
race-condition comment so the durability boundary is well-defined.

      f.write_all(line.as_bytes())
          .map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
+     f.sync_all()
+         .map_err(|e| format!("sync {path:?} 失败：{e}"))?;
      Ok(())


─── src-tauri/src/evolution/change/record.rs:64-72 ───
[bug · medium] `EvalResult` exposes raw `f64` for all five metrics with no validation. Upstream code
that divides by zero, computes `mean / count` over an empty slice, or otherwise produces NaN/±Inf
will trip serde_json's non-finite rejection and surface here as a confusing "序列化失败" inside `append`
— or, if a `serde_json` config is ever relaxed, will silently emit `null` and round-trip into
`None`, corrupting the on-disk record and breaking every downstream consumer that assumes the metric
is present and finite. Either validate in `append`
(`record.eval_before/eval_after.as_ref().map(check_all_finite)`), wrap the fields with a `FiniteF64`
newtype that errors on non-finite construction, or use `Option<f64>` to model "missing/uncomputable"
explicitly.

- #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
- pub struct EvalResult {
-     pub task_success_rate: f64,
-     pub tool_call_efficiency: f64,
-     pub behavior_deviation: f64,
-     pub rollback_rate: f64,
-     pub pollution_survival_days: f64,
-     pub evaluated_at_ms: i64,
+ fn ensure_finite(e: &EvalResult) -> Result<(), String> {
+     for (k, v) in [
+         ("task_success_rate", e.task_success_rate),
+         ("tool_call_efficiency", e.tool_call_efficiency),
+         ("behavior_deviation", e.behavior_deviation),
+         ("rollback_rate", e.rollback_rate),
+         ("pollution_survival_days", e.pollution_survival_days),
+     ] {
+         if !v.is_finite() {
+             return Err(format!("EvalResult.{k} 非有限值：{v}"));
+         }
+     }
+     Ok(())
  }


─── src-tauri/src/evolution/change/record.rs:19-20 ───
[bug · medium] `change_id` uniqueness is not enforced anywhere in this module. The doc says it is
derived as "chg-" + `proposal_id`, but `append` performs no check, the jsonl format has no key
constraint, and `find_by_id` returns `Option<(usize, &ChangeRecord)>` — i.e. the *first* matching
record. If the upstream caller ever replays a proposal, suffers a `proposal_id` collision across two
distinct proposals, or simply retries after a partial-write failure, the duplicate will silently
coexist and any "find latest by id" query returns the older record while the newer one is invisible.
Either enforce uniqueness in `append` (load existing records, refuse on duplicate, or maintain an
in-memory `HashSet<String>` of seen ids in the long-running process), or change `find_by_id`'s
signature to surface `Vec<&ChangeRecord>` so callers can detect duplicates explicitly.

-     /// "chg-" + proposal_id（构造时派生）
-     pub change_id: String,
+ // In append, after acquiring the exclusive flock:
+ let existing = crate::evolution::read_jsonl(path, "changes")?;
+ if existing.iter().any(|r| r.change_id == record.change_id) {
+     return Err(format!("change_id {} 已存在，拒绝追加", record.change_id));
+ }


─── src-tauri/src/evolution/change/record.rs:102-105 ───
[maintainability · low] `ChangeStatus::Pending` (state-machine position) and
`ApprovalSource::Pending` (approval provenance) both serialize to the literal string `"pending"` via
`as_str()` and serde. In flat log lines, grep queries, or key=value records (e.g.
`format!("status={} approval={}", status.as_str(), approval.as_str())`) the two become
indistinguishable — "all pending approvals" cannot be separated from "all records currently in the
pending status", and any aggregation keyed only on the bare string loses the discriminator. Since
`ApprovalSource::Pending` is only meaningful while `ChangeStatus::Pending`, the cleaner fix is to
model the approval as `Option<ApprovalDecision>` (with `None` meaning "no decision yet"), so the
literal `"pending"` appears at most once on the wire; otherwise, prefix the strings
(`status:pending` vs `approval:pending`) so log/search keys are unambiguous.



─── src-tauri/src/evolution/derive.rs:56-57 ───
[bug · high] The module/function docs claim a pure-function contract ("纯函数，输入相同 → 输出相同，dedup 前提"),
but `chrono::Utc::now().timestamp_millis()` makes `created_at_ms` non-deterministic across calls.
Tests only assert `proposal_id` stability, so the broader claim is silently broken — re-derivations
of identical inputs will produce proposals with different `created_at_ms`, and any downstream
consumer using timestamp for dedup/ordering will see drift. Either accept a clock/`now_ms` parameter
(so callers can fix it for tests and pass a stable value when determinism matters) or soften the doc
to "proposal_id is stable; created_at_ms is wall-clock at derivation time".

+     // Pure-function contract: caller passes `now_ms` so the proposal set is fully
+     // deterministic for the given (ops, report, now_ms) triple. This is what dedup relies on.
      let mut out: Vec<EvolutionProposal> = Vec::new();
-     let now_ms = chrono::Utc::now().timestamp_millis();


─── src-tauri/src/evolution/derive.rs:101-108 ───
[security · high] `truncate_chars` only bounds length and never sanitizes — raw op `content` is
concatenated verbatim into `suggestion.text`. The companion test fixture
`derive_evidence_summary_does_not_leak_raw_paths` itself uses `/Users/renshi/secret/file.txt
contains PII` as content, demonstrating that user paths, secrets, or PII can flow straight into the
proposal's user-facing text (and from there to any persisted/logged artifact). `evidence.summary` is
neutral, but `suggestion.text` is not, so downstream persistence or logging of the proposal can
exfiltrate sensitive data that the producer deliberately scrubbed from the summary. Sanitize content
(e.g., redact absolute paths / known PII patterns) before embedding, or document the threat model
and require downstream consumers to redact.

                  suggestion: Suggestion {
                      text: format!(
                          "LLM suggested merging {} entries: {}",
                          ids.len(),
-                         truncate_chars(content, SUGGESTION_MAX_CHARS)
+                         truncate_chars(&redact_sensitive(content), SUGGESTION_MAX_CHARS)
                      ),
                      structured_patch: None,
                  },


─── src-tauri/src/evolution/derive.rs:140-146 ───
[security · high] Same PII/sensitive-content leakage as the Merge branch — raw `content` flows
verbatim into `suggestion.text` (only length-truncated), while the summary is sanitized. Downstream
persistence/logging of the proposal can leak user paths, tokens, or PII. Sanitize before embedding,
or document that consumers must redact.

                  suggestion: Suggestion {
                      text: format!(
                          "LLM resolved contradiction (keep vs drop): {}",
-                         truncate_chars(content, SUGGESTION_MAX_CHARS)
+                         truncate_chars(&redact_sensitive(content), SUGGESTION_MAX_CHARS)
                      ),
                      structured_patch: None,
                  },


─── src-tauri/src/evolution/derive.rs:174-181 ───
[security · high] Same PII/sensitive-content leakage as the other branches — raw `content` flows
verbatim into `suggestion.text`, while the summary is sanitized. Downstream persistence/logging of
the proposal can leak user paths, tokens, or PII. Sanitize before embedding, or document that
consumers must redact.

                  suggestion: Suggestion {
                      text: format!(
                          "LLM distilled {} entries into a rule: {}",
                          ids.len(),
-                         truncate_chars(content, SUGGESTION_MAX_CHARS)
+                         truncate_chars(&redact_sensitive(content), SUGGESTION_MAX_CHARS)
                      ),
                      structured_patch: None,
                  },


─── src-tauri/src/evolution/derive.rs:59-66 ───
[bug · medium] The cap break is order-dependent: the first 5 emit-eligible ops in input order win,
regardless of impact or category priority. Combined with non-deterministic `created_at_ms`, two
consolidation rounds that produce the same logical ops in different orders can yield different
proposal sets — breaking the "pure / dedup-stable" premise at the round level even if individual
proposal_ids stay stable. Either (a) sort ops by impact (Contradiction > Merge > Distill) before
emitting so the cap is priority-stable, or (b) make the priority policy explicit in the doc and
require callers to pre-sort.

-     for op in ops {
+     // Stable cap semantics: sort by impact priority so the same logical round
+     // produces the same proposal set regardless of op ordering.
+     let mut ordered: Vec<&ConsolidateOp> = ops.iter().collect();
+     ordered.sort_by_key(|op| match op {
+         ConsolidateOp::Contradiction { .. } => 0,
+         ConsolidateOp::Merge { ids, .. } if ids.len() >= 3 => 1,
+         ConsolidateOp::Distill { ids, .. } if ids.len() >= 5 => 2,
+         _ => 3,
+     });
+     for op in ordered {
          if out.len() >= MAX_PROPOSALS_PER_ROUND {
              break;
          }
          if let Some(p) = derive_one(op, now_ms) {
              out.push(p);
          }
      }


─── src-tauri/src/evolution/emit.rs:102-105 ───
[bug · high] Mutex-poison recovery via `e.into_inner()` silently keeps using state that another
thread panicked while mutating. If the previous holder panicked mid-insert (e.g. allocation failure,
audit_event! panic) the dedup map can be left with a half-applied mutation or a missing entry; this
code will treat that as authoritative and either lose future dedup hits or drop a fresh proposal
because the previous partial insert left a stale key. Production should at minimum clear/rebuild the
map on poison or fail-fast; the current path quietly masks a real invariant violation.

          let mut g = emitted_map().lock().unwrap_or_else(|e| {
-             eprintln!("[mutex_poisoned] evolution::emit::emitted_map: {e:?}");
-             e.into_inner()
+             eprintln!("[mutex_poisoned] evolution::emit::emitted_map: {e:?}; rebuilding dedup map");
+             e.into_inner().clone() // or: clear and rebuild, then re-lock
          });


─── src-tauri/src/evolution/emit.rs:106-107 ───
[bug · medium] `now_ms - *ts` is a signed i64 subtraction with no overflow protection. In debug
builds this panics on overflow; in release it wraps. Even though stored timestamps are normally
close to `now_ms`, NTP step / manual clock adjustment / a future stored timestamp can produce a
large negative or wrap to i64::MIN and panic the retain closure, taking down `emit_proposals` for
every concurrent caller. Use `saturating_sub` (or `checked_sub`) so clock skew cannot crash the
deduplication path.

-         // 清掉超过 24h 的 id（防止 map 无限增长）
-         g.retain(|_, ts| now_ms - *ts < EMIT_DEDUP_TTL_MS);
+         // 清掉超过 24h 的 id（防止 map 无限增长）；saturating_sub 防 NTP/时钟回退触发 i64 溢出
+         g.retain(|_, ts| now_ms.saturating_sub(*ts) < EMIT_DEDUP_TTL_MS);


─── src-tauri/src/evolution/emit.rs:121-124 ───
[bug · medium] Crash between the dedup-map insert (under the lock) and the subsequent `audit_event!`
(outside the lock) permanently loses that audit row — the next call sees the id in the map and
silently dedup-skips it. The comment frames this as an "acceptable" trade-off but, for an audit
trail, silently dropped rows can be far more costly than the alternative (a duplicate audit line
after a crash). Consider writing the audit row first and only inserting the id on success, or
batching audit + dedup-mark into a single recoverable sink, so the at-most-once invariant holds for
`bot.log` rather than only for the in-process map.

-     let app = APP_HANDLE.get(); // Option<&AppHandle<tauri::Wry>>
- 
+     // 建议顺序反转：先写 audit，确认返回后再插入 dedup；
+     // 崩溃窗口从"标记了没写"变成"写了没标记"（下次重写一次），对 audit 完整性更安全。
      for p in &to_emit {
          if let Some(app) = app {
+             crate::audit_event!( /* ... */ );
+             // 仅写成功后再 insert(proposal_id, now_ms)
+         }


─── src-tauri/src/evolution/emit.rs:114-116 ───
[bug · medium] `EmitReport::written` is incremented when the proposal is inserted into the dedup
map, *before* the audit_event! call. When `APP_HANDLE` is None the audit write is skipped via
`eprintln!` but `written` is still 1, and even when audit_event! itself fails/panics the count is
already updated. Callers will treat this field as "rows actually written to bot.log", which is
false. Either rename to `dedup_marked` and add a separate `audit_emitted` counter, or move the
increment to after a confirmed successful audit write.

              g.insert(p.proposal_id.clone(), now_ms);
-             report.written += 1;
              to_emit.push(p);


─── src-tauri/src/evolution/emit.rs:326-328 ───
[test · medium] The audit-write branch (`crate::audit_event!(...)` with a real AppHandle) is never
exercised: tests cannot construct an `AppHandle<tauri::Wry>`, and `register_app_handle` uses
`OnceLock::set` which only succeeds once and is consumed by production. As a result the field-name
mismatches, payload schema ("proposal_id"/"category"/...), and `eprintln` skip path are all untested
— exactly the parts most likely to silently break the `grep evolution.proposal bot.log` contract the
comment warns about. Either expose a trait/feature that lets tests inject an in-memory audit sink,
or add an integration test that boots the Tauri builder and asserts on captured events.

-     #[test]
-     fn emit_dedup_works_without_app_handle() {
-         let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| {
+     // 建议：抽出 trait AuditSink { fn write(&self, level, event, kv); }
+     //      production 用 tauri-backed impl，tests 用 in-memory Vec impl，
+     //      这样 register_app_handle 与 audit_event! 路径都可被单测覆盖。


─── src-tauri/src/evolution/emit.rs:63-66 ───
[maintainability · low] `register_app_handle(app: AppHandle<tauri::Wry>)` and `static APP_HANDLE:
OnceLock<AppHandle<tauri::Wry>>` hard-bind this module to a specific Tauri runtime. The comment
justifies it as "production only has one", but the concrete parameter type prevents: (a) integration
with mobile/headless test harnesses that use a different runtime, (b) substituting a fake app handle
in tests (the reason the audit path is uncovered — see other comment), and (c) compiling this module
for a non-Wry target. Use a thin newtype (`pub struct EvolutionAppHandle(pub
AppHandle<tauri::Wry>)`) or generic over runtime so the surface area is decoupled.

- /// 类型固定 `AppHandle<tauri::Wry>`：Tauri 默认 runtime 是 Wry，
- /// production 全局只有这一个。测试路径不调本函数（见 `emit_proposals` 解耦说明）。
- pub fn register_app_handle(app: AppHandle<tauri::Wry>) {
-     if APP_HANDLE.set(app).is_err() {
+ // 用 newtype 解耦具体 runtime：
+ // pub struct EvolutionAppHandle(pub AppHandle<tauri::Wry>);
+ // pub fn register_app_handle(app: EvolutionAppHandle) { ... }
+ // 这样未来换 runtime（如 mobile test）或注入 fake 都不破坏调用点。


─── src-tauri/src/evolution/mod.rs:235-241 ───
[other · high] Lock scope is narrower than the comment claims. `lock_evolution_store()` is released
before `apply::apply_from_consolidation(gated)` and `emit::emit_proposals(proposals)` run, so a
concurrent panel rollback/toggle (which DOES take the lock) or a second concurrent
`post_consolidation` can interleave: the proposals JSONL can be rolled back or rewritten while apply
is still writing the corresponding lesson, or two consolidations can both run the apply loop and the
emit loop. Either extend the guard across these calls (or add a dedicated second lock if the intent
is to never block apply on a panel write), or document explicitly that the lock only covers the
JSONL RMW window and rely on evo:<id> idempotency for apply safety.



─── src-tauri/src/evolution/mod.rs:168-168 ───
[other · medium] `auto_allowed` is read once at function entry but reused at two decisions (the
`to_notify` filter and the `if auto_allowed { apply ... }` gate). A concurrent panel toggle between
this read and the apply call will not be observed: apply may run under a confirm policy (or be
skipped under auto) and no audit event will mark the discrepancy. The inline comment acknowledges
the read-once rationale but does not close the race. Consider re-reading `auto_apply_allowed`
immediately under the store lock (just before apply), or wrap the snapshot+gate in the same critical
section so panel toggles are serialized.



─── src-tauri/src/evolution/mod.rs:137-152 ───
[maintainability · medium] `notify_evolution_proposals` opens a fresh DB connection per call and
runs multiple `notif_insert` calls with `?` short-circuiting on the first error: prior inserts are
not rolled back, so a failure midway leaves the notifications table in a partial state and
`emit_changed` may either fire or not based on partial state. Wrap the loop in a single transaction
(or otherwise make the multi-insert atomic). Also, the
`e.suggestion_text.chars().take(60).collect()` / `e.summary.chars().take(120).collect()` plus
`format!("evo:{}", ...)` and `serde_json::json!({...})` allocate a fresh `String`/JSON per entry; if
this path is hot enough to matter, pre-size `String` with `String::with_capacity` or use
`char_indices().nth(...)` slicing against the existing buffer.



─── src-tauri/src/evolution/mod.rs:66-90 ───
[other · medium] The backup path has a TOCTOU window: after detecting corruption, the code does `if
backup.exists()` then `std::fs::copy(path, &backup)`. Between those two steps a concurrent appender
can extend the source file, and the `.corrupt` snapshot will be smaller than the file subsequent
readers see, so forensic value diverges from on-disk state. Also, `std::fs::copy` does not fsync, so
the snapshot can be lost on power loss before the loop continues. At minimum, snapshot via
rename-after-write-temp-and-fsync (write to a unique `.corrupt.<pid>.<ts>`, fsync, rename over the
well-known name) or skip the well-known name entirely and use a unique-per-incident name so each
corrupt line gets its own snapshot.



─── src-tauri/src/evolution/mod.rs:194-199 ───
[bug · high] Split-brain risk between the proposals JSONL and `mem_items`. The store lock only
covers `candidate::write_proposals(app, &proposals)`; once that succeeds,
`apply::apply_from_consolidation(gated)` runs unlocked, and its return value is not even inspected
here — apply can fail or panic and the caller has no way to react (the audit row
`evolution.apply_deferred` only fires on the confirm branch, not on a real apply failure). The
`gated` vec is also captured before the lock is taken, so a concurrent rollback can delete the
proposal from the JSONL between derive and apply, while the apply still writes a lesson for the
now-deleted proposal. Either (a) move `apply_from_consolidation` inside the lock window with the
proposals write so they commit together, or (b) at minimum capture apply's result and emit an
`evolution.apply_failed` audit event with the proposal ids so operators can detect the divergence.



─── src-tauri/src/evolution/observe/stop.rs:75-85 ───
[bug · high] The local definition of "completed" (Active | Rejected | RolledBack | Expired)
intentionally diverges from `ChangeStatus::is_terminal()` (which excludes Active per the in-file
comment). This dual notion of "terminal" is a real footgun: callers (e.g. an aggregation CLI or a
future scheduler) that wire `is_terminal()` into the same dashboards will silently disagree with
this counter (e.g. a still-live Active change counts here but is filtered out by `is_terminal()`).
Consider either (a) naming the local concept differently ("post-Proposed" / "settled") so the
divergence is obvious at call sites, or (b) computing completed via `is_terminal()` plus an explicit
`Active` predicate so there is exactly one source of truth for terminal-vs-not.

  .filter(|c| {
              in_window(c)
-                 && matches!(
-                     c.status,
-                     ChangeStatus::Active
-                         | ChangeStatus::Rejected
-                         | ChangeStatus::RolledBack
-                         | ChangeStatus::Expired
-                 )
+                 && (c.status.is_terminal() || c.status == ChangeStatus::Active)
          })
          .count() as u64;


─── src-tauri/src/evolution/observe/stop.rs:95-100 ───
[other · medium] A reversed observation window (`now_ms < start_ms`) is only announced via
`eprintln!` from inside this library function, then silently coerced to 0 days elapsed so the
time-gate never trips. Two issues: (1) `eprintln!` in a library function is an anti-pattern; the
function should return the condition (e.g. an `InvalidWindow` variant on `StopReason` or a dedicated
`negative_window: bool` on `StopConditionStatus`) so the binary/CLI can decide how to surface it,
and (2) silently suppressing the time gate means a clock/parameter bug in the caller is masked: the
observer keeps running indefinitely. Even if `Result` is too invasive (as the comment notes), at
minimum the status struct should carry the negative-window flag so the CLI can abort with a clear
diagnostic instead of letting R6 A run forever.

- if now_ms < start_ms {
-         eprintln!(
-             "[evolution_stop] negative observation window: now_ms={now_ms} < start_ms={start_ms}（按 0.0 天计，请核时钟/参数）"
-         );
+ let raw_ms = now_ms.saturating_sub(start_ms);
+     let (days_elapsed, negative_window) = if now_ms < start_ms {
+         (0.0_f64, true)
+     } else {
+         (raw_ms as f64 / 86_400_000.0, false)
+     };
+     let mut stop_reasons = Vec::new();
+     if negative_window {
+         stop_reasons.push(StopReason::InvalidWindow);
      }
-     let days_elapsed = (now_ms.saturating_sub(start_ms) as f64 / 86_400_000.0).max(0.0);


─── src-tauri/src/evolution/observe/stop.rs:100-100 ───
[performance · low] `now_ms.saturating_sub(start_ms) as f64` loses precision in the overflow path:
`i64::MAX` (2^63) rounds to `9.223372036854776e18` (≈1 ULP), and any non-saturating i64 near that
range silently loses low bits. Functionally the gate still trips today (the test asserts `>
1_000_000.0` days), but the precision loss is undocumented and would silently desync if anyone (a)
lowers `STOP_MAX_DAYS`, (b) switches the unit to seconds/ns, or (c) compares `days_elapsed` against
a value close to the saturation point. At minimum, document the saturation/precision contract in the
doc comment so future changes to the threshold or unit don't silently misbehave; or compute via
`(now_ms / DAY_MS) - (start_ms / DAY_MS)` style integer arithmetic that avoids the lossy cast.

+ // NOTE: `as f64` rounds i64::MAX down to ~9.22e18 (~1 ULP). The resulting
+     // day count (~1.07e14) is still >> 14, so the gate trips; if STOP_MAX_DAYS
+     // or the unit ever changes, prefer integer-day arithmetic here.
- let days_elapsed = (now_ms.saturating_sub(start_ms) as f64 / 86_400_000.0).max(0.0);
+     let days_elapsed = (now_ms.saturating_sub(start_ms) as f64 / 86_400_000.0).max(0.0);


─── src-tauri/src/evolution/proposal.rs:140-146 ───
[bug · medium] `normalize_for_hash` only maps ASCII digits to `N`; Unicode digits (Arabic-Indic ٠-٩,
full-width ０-９, CJK 一二三四五六七八九〇, etc.) are not caught by `is_ascii_digit()` but do match
`is_alphanumeric()`, so they fall through to the `else if c.is_alphanumeric()` branch unchanged.
This silently breaks the documented "所有数字 → N" contract: e.g. "error ٣" vs "error 3" produce
different proposal_ids, defeating dedup for non-ASCII-digit summaries. Either document the
ASCII-only scope explicitly, or widen the digit check (e.g. `c.is_ascii_digit() || c.is_numeric()`
followed by an alphanumeric-but-not-numeric gate), so the normalization truly collapses numeric
tokens across scripts.

  let mapped = if c.is_ascii_digit() {
+             'N'
+         } else if c.is_numeric() {
+             // Non-ASCII digits (Arabic-Indic, full-width, CJK, …):
+             // map to 'N' too so "error ٣" and "error 3" dedup.
              'N'
          } else if c.is_alphanumeric() {
              c.to_ascii_lowercase()
          } else {
              ' '
          };


─── src-tauri/src/evolution/proposal.rs:175-183 ───
[maintainability · medium] `is_reversible` is MVP and treats everything except `ToolSchemaHint` and
`High` impact as reversible. The downstream shadow/activation routing trusts this signal, but
`MemoryPolicy` and `PromptSection` mutations at Medium impact can rewrite prompts or memory rules
that affect every future LLM call — not safely "rollback-able" without snapshotting the prior
artifact. The comment marks it as a deliberate simplification ("R7→A 简化"), so this is intentional,
but worth either documenting the snapshot/rollback contract that the caller must uphold, or
extending the rule (e.g. also block `ProposalTarget::PromptSection {..}` and
`ProposalTarget::MemoryPolicy {..}` unless a snapshot exists) before Phase 2 routes any irreversible
action through it.

  pub fn is_reversible(p: &EvolutionProposal) -> bool {
      if matches!(p.category, ProposalCategory::ToolSchemaHint) {
          return false;
      }
      if matches!(p.impact, ImpactLevel::High) {
          return false;
      }
-     true
+     // Medium/Low impact changes to prompts or memory policy are only
+     // safely reversible if a snapshot of the prior artifact is retained.
+     // Callers must guarantee that snapshot exists before relying on `true`.
+     match (&p.category, &p.target) {
+         (ProposalCategory::PromptHint, _) => false,
+         (_, ProposalTarget::PromptSection { .. }) => false,
+         (_, ProposalTarget::MemoryPolicy { .. }) => false,
+         _ => true,
+     }
  }


─── src-tauri/src/evolution/proposal.rs:106-116 ───
[security · medium] The privacy rule on `Evidence` — "不存原始堆栈 / 文件路径 / 用户输入" — is enforced only by
doc comments. `summary: String` and `related_refs: Vec<String>` accept arbitrary content with no
length cap, no allowlist, and no redaction pass; the existing tests don't probe for path/PII leakage
either, so a future audit-log writer that accidentally forwards a raw `Display` of an error chain or
an absolute filesystem path will silently land PII / secrets into the audit file. Either tighten the
type (e.g. a `RedactedText` newtype whose `Display` strips `/`, `\`, `:`, and long tokens; cap
`summary` length; restrict `related_refs` to opaque id-only strings via a typed `RefId` enum), or
add a unit test that fails when a known-path sample is fed through.

+ /// 证据（写 audit 用，所有物理/路径类数据已被 redact）。
  pub struct Evidence {
      /// 一句话描述发现（自然语言，**不**存原始堆栈 / 文件路径 / 用户输入）。
-     pub summary: String,
-     /// 发生次数。Phase 1 语义：单次反思的 ops 内 ids 数（占位语义，非跨次累计）——
-     /// 见 HANDOFF.md，避免一周后误读为「同类失败出现 N 次」。
+     pub summary: RedactedText,
      pub occurrence_count: u32,
-     /// 时间窗口（小时）。Phase 1 固定 24h。
      pub window_hours: u32,
      /// 关联引用：trace_id / tool_name / memory id（**不**存原始路径 / 对话内容）。
-     pub related_refs: Vec<String>,
+     pub related_refs: Vec<OpaqueRef>,
+ }
+ 
+ /// 已经被 redact 过的纯文本（构造时裁掉绝对路径 / 长 token / 控制字符）。
+ #[derive(Debug, Clone, Serialize, Deserialize)]
+ #[serde(transparent)]
+ pub struct RedactedText(String);
+ 
+ /// 关联引用只能是已知形态的 opaque id，序列化形态保持稳定。
+ #[derive(Debug, Clone, Serialize, Deserialize)]
+ #[serde(tag = "kind", rename_all = "snake_case")]
+ pub enum OpaqueRef {
+     TraceId { value: String },
+     ToolName { value: String },
+     MemoryId { value: String },
  }


─── src-tauri/src/evolution/observe/synthetic.rs:187-191 ───
[performance · medium] O(n·m) lookup inside the promoted loop. For every promoted proposal (≈60),
`proposals.iter().find(|p| &p.proposal_id == id)` linearly scans all `proposal_total` entries (100),
making change/record generation O(P²). More importantly, the `.unwrap_or(now_ms - MS_PER_DAY)`
silently masks the "impossible" branch — if the lookup ever fails, the generated change gets a fake
created_at_ms that desynchronizes the timeline tests below. Either build a `HashMap<&str, i64>` of
id→created_at_ms during step 1, or store `created_at_ms` alongside `promoted_proposal_ids` as
`Vec<(String, i64)>`.



─── src-tauri/src/evolution/observe/synthetic.rs:128-129 ───
[bug · medium] `now_ms` is not validated. `validate()` checks `window_days` bounds and the
`window_days * MS_PER_DAY` overflow, but never checks `now_ms` itself. A caller passing `now_ms = 0`
(or any value < `window_days * MS_PER_DAY`) yields a negative `window_start_ms`, which then
propagates into `created_at_ms`, `expires_at_ms`, the `rolled_back_at`/`.max(created_at_ms)` clamps,
and the `applied_at_ms` clamp. Downstream R6 metric code that does window comparisons on
`applied_at_ms` will silently misbehave. Add a `now_ms >= window_days * MS_PER_DAY` guard (or use
`checked_sub`) so the failure surfaces as a typed error instead of corrupt timestamps.



─── src-tauri/src/evolution/observe/synthetic.rs:126-129 ───
[bug · medium] `now_ms - cfg.window_days * MS_PER_DAY` can overflow in debug mode (panic) or wrap in
release mode if a caller passes `now_ms = i64::MIN`. `validate()` ensures `window_days * MS_PER_DAY`
fits in i64, but does not ensure the subtraction stays non-negative or within i64 range. Use
`checked_sub` / a `saturating_sub` or extend `validate()` with `now_ms >= window_days * MS_PER_DAY`
so the contract is explicit.



─── src-tauri/src/evolution/observe/synthetic.rs:307-312 ───
[performance · medium] The LCG combined with `%` produces biased, low-entropy draws that undermine
the stated purpose of "验证 R6 指标的计算逻辑（尺子准不准）". (1) `next_int` uses classic modulo bias — `next_u64()
% max_exclusive` over-represents low residues — so the `pick_layer`/`pick_impact`/status buckets
aren't exactly the configured ratios. (2) `next_f64` only yields 100_000 distinct values in [0,1),
so boundary comparisons like `r < 0.60` snap to a 1e-5 grid. Combined with the ±5/±10 tolerances in
`distribution_reasonable` / `promoted_count_matches_config_ratio`, real distribution drift is hidden
— a metric-validation fixture should make these guarantees explicit (use a wider RNG like
`xorshift`, apply unbiased rejection sampling for `next_int`, and tighten test tolerances).



─── src-tauri/src/evolution/observe/synthetic.rs:500-503 ───
[test · medium] Temp-dir name uses `chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)`. On
32-bit targets `timestamp_nanos_opt` returns `None` and the fallback `0` makes the directory name a
constant `synth-0` — two concurrent `cargo test` runs (or two same-nanosecond invocations) will
collide on the same temp dir, and the final `remove_dir_all` will delete the other run's working
tree. Mix in `std::process::id()` and an atomic counter, or pull in the `tempfile` crate for a
guaranteed-unique scratch directory.



─── src-tauri/src/evolution/sandbox/io.rs:0-0 ───
[other · medium] Reader–writer synchronization gap is intentional but only half-documented.
`read_shadow`/`read_ab` skip `SANDBOX_IO_LOCK`, so a read started during an append can race with the
writer; because each append is one atomic `write_all` on POSIX O_APPEND, the reader never sees a
torn line, but the ordering guarantee ("what I read is a consistent snapshot from some moment")
isn't stated. Either tighten with a `RwLock`/`read` lock or make the rationale (atomic O_APPEND +
stale snapshots acceptable) explicit so a future maintainer doesn't silently change the contract.



─── src-tauri/src/evolution/sandbox/io.rs:225-230 ───
[test · high] Hardcoded `/tmp/...` paths break on Windows and target a world-writable directory on
POSIX. `std::path::Path::new("/tmp/...")` won't resolve on non-Unix targets, and even on Linux a
hostile user could create these files between iterations. Mirror the other tests and build the path
from `std::env::temp_dir()` with a unique suffix so the test suite still passes in restricted
sandboxes / CI runners / Windows builds.



─── src-tauri/src/evolution/sandbox/io.rs:190-193 ───
[test · medium] `timestamp_nanos_opt().unwrap_or(0)` is a weak uniqueness source. With `cargo test`
running tests in parallel and on fast CI a few of them can hit the same nanosecond (or overflow into
`None`), causing temp-dir collisions and racy cleanup. Mix in `std::process::id()` or an `AtomicU64`
counter (or use `uuid` / `rand`) so concurrent tests always get distinct directories.



─── src-tauri/src/evolution/sandbox/io.rs:250-254 ───
[maintainability · low] `mk_change` exists only to be called from `_unused_silence`, whose sole
purpose is to keep `mk_change` "used" so its `dead_code` warning stays suppressed. The misleading
comment ("沉默未用变量" — silence unused variables) hides a real dead helper. Delete both, or actually
exercise `mk_change` in a meaningful test (e.g., constructing a shadow/ab flow from a
`ChangeRecord`); don't smuggle lint suppression through a dummy caller.



─── src-tauri/src/evolution/observe/shadow.rs:517-522 ───
[bug · medium] The file-level `#![allow(clippy::await_holding_lock)]` plus the comment
"current-thread runtime 故意持跨 await" is fragile. Every `#[tokio::test]` body opens with `let _g =
counter_lock();` (a `std::sync::MutexGuard<'static, ()>`) and the guard is held across the
subsequent `.await`. The justification relies on `#[tokio::test]` defaulting to a single-thread
current-thread runtime — but nothing in code or CI guards this. If anyone adds `#[tokio::test(flavor
= "multi_thread")]`, wraps one of these tests in `tokio::spawn`, or runs them under `#[test]` while
another test holds the lock on a different blocking-pool worker, the `std::sync::Mutex` will
deadlock (it does not yield). The mutex is only there to serialize counter mutation across tests,
which can also be achieved with a `tokio::sync::Mutex` or simply per-test counter local variables —
both of which would eliminate this hazard entirely.

-     // B5-6：await_holding_lock 豁免——测试串行锁（counter_lock 等 std Mutex
-     // guard）**故意**持跨 await：#[tokio::test] 独立 current-thread runtime，
-     // guard 持有至测试结束正是串行化语义，无真实死锁面
-     #![allow(clippy::await_holding_lock)]
-     use super::*;
-     use crate::evolution::proposal::{
+     // 串行化改用 tokio::sync::Mutex——避免 std::sync::Mutex 跨 await 死锁；
+     // 锁粒度只覆盖计数器增量，不覆盖 await 内的业务逻辑
+     use tokio::sync::Mutex as AsyncMutex;
+     static COUNTER_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
+     async fn counter_lock() -> tokio::sync::MutexGuard<'static, ()> {
+         COUNTER_LOCK.lock().await
+     }


─── src-tauri/src/evolution/sandbox/kill_switch.rs:92-97 ───
[bug · high] load_from_file silently mutates the in-memory `KillSwitch` to differ from the on-disk
file when the file has `all_auto_apply=true + shadow_only=false`: it flips `shadow_only` to true
without writing back. The doc-comment rationale ("防配置写入方持久化矛盾态") is therefore not actually achieved
— the file remains in the contradictory state, and any later code path that re-reads the same path
(or a sibling watcher) will see a value that disagrees with what the rest of the process has been
using. Note that the predicates `should_auto_apply`/`should_shadow_only` already handle the
contradiction correctly via `||`, so the normalization is purely cosmetic / a debugging trap. Either
drop the normalization entirely (let the predicates carry the invariant), return a typed `Err` so
the caller fixes the config, or rewrite the file with the normalized value so the on-disk and
in-memory states actually agree.

  if k.all_auto_apply && !k.shadow_only {
-         eprintln!(
-             "[evolution_kill_switch] 矛盾组合 all_auto_apply=true + shadow_only=false，归一化为 shadow_only=true"
-         );
-         k.shadow_only = true;
+         return Err(format!(
+             "kill_switch 矛盾组合：all_auto_apply=true 要求 shadow_only 也为 true（{path:?}）"
+         ));
      }


─── src-tauri/src/evolution/sandbox/kill_switch.rs:93-95 ───
[maintainability · medium] load_from_file's own doc comment says it is invoked on every apply check
("立即生效 = 每次调用现读"). Routing diagnostics through `eprintln!` bypasses whatever structured logging
facility the rest of the crate uses, and if the contradictory config persists the line will spam
stderr on every call. Replace with the crate's logging macro (`tracing::warn!` / `log::warn!`) so
the diagnostic can be filtered, sampled, and routed alongside other warnings.

- eprintln!(
-             "[evolution_kill_switch] 矛盾组合 all_auto_apply=true + shadow_only=false，归一化为 shadow_only=true"
+ tracing::warn!(
+             "evolution_kill_switch 矛盾组合 all_auto_apply=true + shadow_only=false，归一化为 shadow_only=true"
          );


─── src-tauri/src/evolution/sandbox/kill_switch.rs:177-182 ───
[test · medium] Each load_from_file test asserts first and then runs `let _ =
std::fs::remove_dir_all(&dir);` afterwards. If an assertion panics, the cleanup line is skipped and
the temp dir leaks under `std::env::temp_dir()` (shared `/tmp` across the host). Move cleanup into a
small RAII guard constructed immediately after the directory is created so it runs on both success
and panic paths.

+ struct DirGuard(std::path::PathBuf);
+         impl Drop for DirGuard {
+             fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
+         }
+         let _guard = DirGuard(dir.clone());
- let k = load_from_file(&p).unwrap();
+         let k = load_from_file(&p).unwrap();
          assert!(k.all_auto_apply);
          // 矛盾组合（all_auto_apply=true + shadow_only=false）边界归一化：
          // all_auto_apply 隐含 shadow_only
          assert!(k.shadow_only);
-         let _ = std::fs::remove_dir_all(&dir);


─── src-tauri/src/evolution/sandbox/kill_switch.rs:153-156 ───
[test · medium] The temp-dir suffix `chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)` is
unsafe on two axes: two tests created within the same nanosecond (very plausible under `cargo
test`'s parallel runner) get identical suffixes and race on the shared `std::env::temp_dir()`; and
`.unwrap_or(0)` silently collapses any clock/overflow failure into a literal "0", causing every test
in that mode to collide catastrophically. Use a per-process monotonic counter (or
`std::process::id()` plus a counter) so collisions are impossible regardless of clock resolution.

+ use std::sync::atomic::{AtomicU64, Ordering};
+         static SEQ: AtomicU64 = AtomicU64::new(0);
- let dir = std::env::temp_dir().join(format!(
+         let dir = std::env::temp_dir().join(format!(
-             "ks-ok-{}",
-             chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
+             "ks-ok-{}-{}",
+             std::process::id(),
+             SEQ.fetch_add(1, Ordering::Relaxed)
          ));


─── src-tauri/src/evolution/sandbox/kill_switch.rs:80-80 ───
[maintainability · low] load_from_file returns `Result<KillSwitch, String>`, flattening io::Error
and serde_json::Error into a single opaque string. Callers cannot programmatically distinguish "file
missing", "malformed JSON", "missing evolution block", or "missing kill_switch block" — the tests in
this file already work around the loss with `e.contains("kill_switch")` / `e.contains("evolution")`
substring matches, which are fragile to message wording changes. Replace `String` with a `thiserror`
enum so callers (and tests) can match on the underlying cause.

- pub fn load_from_file(path: &Path) -> Result<KillSwitch, String> {
+ #[derive(Debug, thiserror::Error)]
+ pub enum LoadError {
+     #[error("读 {path:?} 失败：{source}")] Io { path: std::path::PathBuf, #[source] source: std::io::Error },
+     #[error("解析 {path:?} 失败：{source}")] Json { path: std::path::PathBuf, #[source] source: serde_json::Error },
+     #[error("bot-config.json 缺少 evolution 块")] MissingEvolution,
+     #[error("evolution 块缺少 kill_switch 子块")] MissingKillSwitch,
+     #[error("kill_switch 解析失败：{0}")] KillSwitchParse(#[source] serde_json::Error),
+     #[error("kill_switch 矛盾组合：all_auto_apply=true 要求 shadow_only 也为 true（{0:?}）")] Contradictory(std::path::PathBuf),
+ }
+ pub fn load_from_file(path: &Path) -> Result<KillSwitch, LoadError> {


─── src-tauri/src/evolution/sandbox/kill_switch.rs:81-81 ───
[security · low] `std::fs::read_to_string` loads the entire config file into memory with no size
cap. Because load_from_file is called on every apply check on a caller-supplied `Path` (the file's
own doc comment says "立即生效 = 每次调用现读"), a symlink-swapped or accidentally huge file at that path will
block or OOM the process without surfacing a typed error. Read `metadata().len()` up front and
reject anything past a sane cap (a few MB is more than enough for bot-config.json), or use
`BufReader::take` to bound the read.

+ const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
+     let meta = std::fs::metadata(path).map_err(|e| format!("读 {path:?} 失败：{e}"))?;
+     if meta.len() > MAX_CONFIG_BYTES {
+         return Err(format!("{path:?} 超过 {} 字节上限", MAX_CONFIG_BYTES));
+     }
- let raw = std::fs::read_to_string(path).map_err(|e| format!("读 {path:?} 失败：{e}"))?;
+     let raw = std::fs::read_to_string(path).map_err(|e| format!("读 {path:?} 失败：{e}"))?;


─── src-tauri/src/evolution/observe/shadow.rs:0-0 ───
[test · high] Production entry shadow_apply_for_batch_with_app is the only path invoked from
apply.rs:383 (per its docstring "R7→A→B 后唯一对外入口"), yet it has ZERO direct test coverage in this
file. Every e2e test (e2e_1..4, e2e_preflight_*, e2e_failed_write_*, e2e_high_failure_rate_*,
e2e_apply_to_shadow_integration, e2e_e4_*) exercises only the trait variants shadow_apply_for_batch
/ shadow_apply_for_batch_with_reversibility, which explicitly bypass the production-only logic:
spawn_blocking + lock_evolution_store() + on-disk dedup via change::read_all + unique_change_id_for
+ S0/S1/S2 audit routing + per-proposal failure-rate alert. All production-only branches
(Appended/Deduped/Error arms, the three (state, s2_decision) audit branches, the dedup hit counter,
and the production failure-rate check at function tail) are unverified. Suggest at minimum adding an
in-process integration test that routes production through ShadowSink so the existing MockShadowSink
e2e tests actually cover this path.



─── src-tauri/src/evolution/observe/shadow.rs:224-226 ───
[bug · high] Failure-rate alert reads TOTAL_WRITES and FAILED_WRITES with Ordering::Relaxed in TWO
separate load() calls. Under any concurrent shadow batch (or concurrent shadow + panel rewrite +
apply append on changes.jsonl), the two loads can straddle an increment on either counter, producing
a stale or temporarily out-of-range ratio. More importantly, the production path increments them
per-proposal inside the loop, then computes the ratio from cumulative state, so the alert is "ever
exceeded 5% since startup", not "this batch exceeded 5%" (the comment claims the latter). Also the
ratio can briefly exceed 1.0 if FAILED_WRITES is observed briefly larger than TOTAL_WRITES after
interleaved increments — harmless numerically but evidence the read pair is unsynchronized. Either
drop the cumulative model (track per-call failed/total locally and compare against a separate
lifetime counter) or make the semantic explicit in the field name / doc, and pair the two loads with
Ordering::Acquire/Release so the ratio is at least point-in-time consistent.



─── src-tauri/src/evolution/observe/shadow.rs:176-183 ───
[maintainability · medium] ShadowReport.deduped is part of the public struct documented as "B2-3: 同
proposal_id 已有未终态 CR 而跳过的条数（去重命中）", but the two generic entry points shadow_apply_for_batch and
shadow_apply_for_batch_with_reversibility unconditionally populate it as 0 — only
shadow_apply_for_batch_with_app computes a real value. Callers consuming the trait variants get
silently-wrong data (a value of 0 looks like "no dedup happened" rather than "this path doesn't
dedup"). Suggest either: move dedup into a shared ShadowSink::write_change so trait variants get
real numbers too; split into ShadowReport (always populated) vs ShadowReportWithDedup (production
adds the field); or change deduped to Option(usize) where None means "not tracked".



─── src-tauri/src/evolution/observe/shadow.rs:517-521 ───
[bug · medium] The file-level allow(clippy::await_holding_lock) plus the comment "current-thread
runtime 故意持跨 await" is fragile. Every tokio::test body opens with let _g = counter_lock(); (a
std::sync::MutexGuard of 'static ()) and the guard is held across the subsequent .await. The
justification relies on tokio::test defaulting to a single-thread current-thread runtime — but
nothing in code or CI guards this. If anyone adds tokio::test(flavor = "multi_thread"), wraps one of
these tests in tokio::spawn, or runs them under plain #[test] while another test holds the lock on a
different blocking-pool worker, the std::sync::Mutex will deadlock (it does not yield). The mutex
only exists to serialize counter mutation across tests — which can also be achieved with a
tokio::sync::Mutex (await-aware) or per-test local counter variables, both of which eliminate the
hazard entirely.



─── src-tauri/src/evolution/observe/shadow.rs:380-395 ───
[performance · medium] Inside the per-proposal spawn_blocking closure, change::read_all(&path)
re-reads the ENTIRE evolution-changes.jsonl from disk on every iteration. For a batch of N proposals
this is O(N × file_size) I/O under the held EVOLUTION_STORE_LOCK, which serializes all writers
(shadow + panel + apply) for the duration. For typical small files / small batches this is fine, but
it scales poorly and amplifies contention on the global lock. Two cheap fixes: (a) read rows once
OUTSIDE the loop under the same lock and pass an owned snapshot into spawn_blocking (loses
intra-batch concurrency but big I/O win); (b) keep per-iteration read_all but maintain an in-memory
dedup index across calls (e.g., a HashSet of seen proposal_ids seeded once). Either way, the B2-3
dedup check needs the SAME lock window, so the read must happen inside the locked region — the
question is only whether you read once-per-batch or once-per-proposal.



─── src-tauri/src/evolution/observe/shadow.rs:407-411 ───
[bug · medium] The comment "tauri::Error (非 tokio JoinError) 无 is_panic/into_panic——Display 已带 panic
payload 信息，直接透传" is INCORRECT. tauri::async_runtime::spawn_blocking returns Result(T, tauri::Error),
and tauri::Error does NOT preserve the panic payload via Display (it has no is_panic/into_panic
API). If change::from_proposal, change::read_all, change::append_change, or lock_evolution_store
panics inside the closure, the original panic location / message / backtrace are dropped on the
floor and the caller only sees the generic "shadow 写线程 join 失败：{e}" string. This is the production
hot path with the new B2-3 dedup and B2-4 change-id logic — exactly the logic most likely to panic
on edge cases (corrupt jsonl line in read_all, duplicate change_id collision, etc.). Silently losing
panic context will make prod debugging painful. Suggest wrapping the closure body in
std::panic::catch_unwind to capture the original payload, or switch to tokio::task::spawn_blocking
which exposes JoinError::is_panic.



─── src-tauri/src/evolution/panel/commands.rs:0-0 ───
[bug · high] Doc/code mismatch on lock order: function claims "外层已持 EVOLUTION_STORE_LOCK", but the
only call site is `toggle_inner` 段②, which the surrounding comment explicitly marks as "锁外落库" (no
store lock held). Either the doc is stale and needs to be updated to state "called without
EVOLUTION_STORE_LOCK; takes only DB_WRITE_LOCK", or the call site is missing an outer store-lock
acquisition. The mismatch is a footgun for future maintainers who rely on the doc when reasoning
about deadlock avoidance.



─── src-tauri/src/evolution/panel/commands.rs:369-379 ───
[bug · high] Self-transition hazard in mark_human_applied: walks the fixed sequence `[Shadowing,
ShadowPassed, Approved, Active]` without skipping elements already past. The dedup above
(`matches!(..., Pending | Shadowing | ShadowPassed | Active)`) lets `cr` start at
Shadowing/Shadowed/ShadowPassed — in those cases the first iteration calls
`change::transition(Shadowing, Shadowing)` (or `transition(ShadowPassed, ShadowPassed)`), which
`status.rs` 硬约束 may reject. The existing `w1_mark_human_applied_walks_legally_and_idempotent` test
only covers `Pending→Active` and `Active→Active`, leaving the mid-flow cases uncovered. Either skip
elements until the current status, or guard with a `match cr.status` index lookup.



─── src-tauri/src/evolution/panel/commands.rs:181-190 ───
[performance · medium] Redundant apply on already-Active dedup hit: when the dedup matches an
existing `Active` CR, segment ② still calls `human_apply_one` and `apply::append_applied_record`.
The safety of this branch rests entirely on `apply::apply_one`'s idempotency keyed by `evo:<pid>`;
segment ③ correctly skips the CR rewrite, but the lesson-write + append_applied_record IO is paid
for every repeat toggle-ON of an already-applied proposal. Consider short-circuiting when
`existing.status == Active` before constructing `apply_target`, or special-casing
`apply::apply_one`'s `AlreadyPresent` to skip the append.



─── src-tauri/src/evolution/panel/commands.rs:357-360 ───
[security · medium] Silent mutex-poisoning recovery (4 sites): `lock().unwrap_or_else(|e| {
eprintln!(...); e.into_inner() })` on `DB_WRITE_LOCK`. If the previous holder panicked
mid-transaction the protected state (jsonl files / SQLite rows) may already be inconsistent;
clearing the poison and continuing writes can compound corruption. Consider failing closed (return
`Err`) on poisoned mutex in production paths, or scoping the recovery to startup-recovery code paths
only and documenting the invariant clearly.



─── src-tauri/src/evolution/panel/commands.rs:410-417 ───
[other · medium] Silent mutex-poisoning recovery in cascade path: same pattern as `human_apply_one`.
If a previous holder of `DB_WRITE_LOCK` panicked mid-write, the memory table may be in a torn state;
silently continuing to delete rows can compound corruption. Fail closed or document the invariant.



─── src-tauri/src/evolution/panel/commands.rs:837-841 ───
[security · medium] Silent mutex-poisoning recovery in rollback path: same pattern. If
`DB_WRITE_LOCK` was held across a panic, the mem_items state could be inconsistent; continuing to
delete can mask corruption.



─── src-tauri/src/evolution/panel/commands.rs:381-382 ───
[other · low] Cascade path holds `EVOLUTION_STORE_LOCK` across DB connection open + `DB_WRITE_LOCK`
acquire + SQLite row deletes. The lock order `store → DB` is consistent with the documented
discipline, but it extends the store-critical section to include disk IO for `open_db` and SQL
execution. A misbehaving DB call (slow query, fsync) will block all other evolution store
operations. Consider opening the DB connection and acquiring `DB_WRITE_LOCK` BEFORE taking
`EVOLUTION_STORE_LOCK`, mirroring the `toggle_inner` 段② pattern (which deliberately does DB work
outside the store lock).



─── src-tauri/src/evolution/sandbox/routing.rs:33-35 ───
[performance · medium] ab_bucket allocates a new String via format!("ab:{session_id}") on every
call. Since the module header declares the interface spec-frozen and is_canary already runs without
allocation (bucket hashes session_id bytes directly), this asymmetry will become hidden per-request
allocator pressure once is_ab_a is wired to production traffic. Either salt inline by iterating over
"ab:".bytes() then session_id.bytes() in fnv1a, or expose a fnv1a_salted(salt: &[u8], key: &[u8])
helper.



─── src-tauri/src/evolution/sandbox/routing.rs:59-67 ───
[test · medium] Test name implies a 'known value' lock but the second half (`assert_eq!(h,
fnv1a("test"))`) is trivially self-equal and never fails — FNV-1→FNV-1a swap, prime change, or
byte-order change all pass. The comment says '锁死一个非空值' but no actual value is locked. Pin to the
real FNV-1a("test") = 0x46c6487d4d2678b9 (or a recomputed constant) so the test actually guards the
algorithm.



─── src-tauri/src/evolution/sandbox/routing.rs:138-143 ───
[performance · low] Orthogonality test constructs format!("session-{i}") twice per iteration in the
first filter and twice in the second (4× per i, 40 000 allocations over the run). Hoist to a single
let per iteration, or use (0..10000).map(|i| format!("session-{i}")).filter(|s| is_canary(s) &&
is_ab_a(s)).count() to allocate once per i. Also avoids the asymmetry that the same key string is
rebuilt redundantly.



─── src-tauri/src/evolution/strategy.rs:41-48 ───
[documentation · medium] The doc comment claims "其余（含缺字段）按 Auto" (unknown values, including missing
field, are treated as Auto), but the function returns `None` for any non-matching string. The
documented default contract is only honored if the caller remembers to apply
`.unwrap_or(Self::Auto)`; if a future caller uses the raw `Option` (or `if let Some(...)`), an
unknown config silently becomes 'no policy' rather than Auto. Either make the function infallible
(return `Self` and default to Auto internally) or rewrite the doc to clarify that `None` is the
function's actual signal for unknown input and is conventionally mapped to Auto by callers.

- /// 配置字符串解析：仅认 "auto" / "confirm"，其余（含缺字段）按 Auto。
+ /// 配置字符串解析：仅认 "auto" / "confirm"；其他字符串返回 `None`，
+     /// 由调用方按 Auto 处理（详见模块文档的不变式）。
      pub fn from_config_str(s: &str) -> Option<Self> {
          match s {
              "auto" => Some(Self::Auto),
              "confirm" => Some(Self::Confirm),
              _ => None,
          }
      }


─── src-tauri/src/evolution/strategy.rs:55-65 ───
[maintainability · medium] `EvalContext` carries impure inputs (`now_ms` clock snapshot +
`KillSwitch`) while the module-level invariant forbids clocks in the policy layer. The trait methods
currently don't consume this type, which is correct — but the boundary is not enforced at the type
level: if a future `EvolutionPolicy` method is extended to take `&EvalContext`, time-based decisions
could silently leak into the supposedly pure strategies. Consider documenting the boundary invariant
on the struct itself (e.g. "only orchestration may read these fields") or introducing a sealed
marker trait so policy-side extensions are caught at compile time.



─── src-tauri/src/evolution/strategy.rs:106-108 ───
[maintainability · low] This is documented as a transitional helper for conflict.rs's "zero-logic
delegation", scheduled for cleanup at "批次 C". However, no callers are visible in this file (only
used by the trait impl below), so it is currently orphan `pub(crate)` API surface. Verify that
conflict.rs actually delegates to this function; otherwise downgrade visibility (or move it into the
trait impl as a private fn) so the cleanup date is not silently missed.



─── src-tauri/src/evolution/strategy.rs:125-127 ───
[maintainability · low] Same transitional-state concern as `layer_priority`: this helper is
`pub(crate)` purely as a delegation surface for conflict.rs and is slated for removal at batch C. No
callers visible in this file. If conflict.rs has not yet been rewired to call this directly, the
visibility should be downgraded now to keep the cleanup scope honest.



─── src-tauri/src/evolution/policy.rs:80-83 ───
[other · medium] Mutex guard spans `std::fs::read_to_string` and `atomic_write` (which typically
fsyncs + renames). On slow disks or under contention this can serialize settings writes for hundreds
of ms each, and any future same-process caller stacking this mutex (e.g., a follow-up writer) will
queue behind the fsync. The RMW invariant is documented as intentional, but consider whether
narrowing the critical section to just the `atomic_write` (after an unlocked read into a local
`Value`) would preserve correctness while reducing lock-held-across-fsync risk — or at minimum
confirm there is no other code path that could deadlock against this lock.



─── src-tauri/src/evolution/policy.rs:80-83 ───
[other · medium] `unwrap_or_else(|e| e.into_inner())` silently continues writing after a poisoned
mutex. If a prior holder panicked between `read_to_string` and `atomic_write`, the in-memory
mutation is lost but the on-disk file is unchanged (atomic_write uses temp+rename, so the file is
never half-written). However the recovery proceeds without re-reading the file, so the next call
still re-reads from the unchanged disk state — which is safe but masks a real risk: if
`atomic_write` itself left a partial temp file behind on panic, the next write could collide with
that leftover. After poisoning recovery, re-validate by parsing the file before proceeding, or treat
poisoning as a hard error requiring operator intervention.



─── src-tauri/src/evolution/policy.rs:44-48 ───
[maintainability · low] Every call with unparseable JSON emits a fresh `eprintln!`. If
`bot-config.json` stays corrupt (e.g., user manually edited and broke it), the diagnostic line will
repeat on every consolidate cycle and from any other reader, turning a single trace line into
persistent log spam in production. Gate to first-occurrence (e.g., `Once` + atomic flag, or a
rate-limited `tracing` event with `once = true`) so the trace fires once until the file is fixed,
rather than once per read.



─── src-tauri/src/evolution/sandbox/shadow.rs:46-47 ───
[maintainability · medium] `would_inject: bool` collapses three semantically distinct outcomes into
two bits: Pass→true, Fail→false, Skipped→false. The Skipped branch carries the explicit note
"no_baseline_lessons" (i.e. "we don't know"), yet the bool cannot distinguish that from a genuine
"no". Per the project rule that domain states should be modeled with enums instead of booleans when
invalid/ambiguous states would otherwise be representable, this field should be `Option<bool>` or,
better, an enum like `WouldInject { Yes, No, Unknown }` (or just drop it and let consumers read
`decision`, since `decision` already encodes Pass/Fail/Skipped).

- /// 该 candidate 会在 injection_block.lessons 出现吗？
-     pub would_inject: bool,
+ /// 该 candidate 会在 injection_block.lessons 出现吗？（unknown 与 no 必须区分）
+     pub would_inject: Option<bool>,


─── src-tauri/src/evolution/sandbox/shadow.rs:101-108 ───
[bug · medium] The Skipped branch always fires when `existing_lessons.is_empty()`, but
`hash_before`/`hash_after` are unconditionally computed *before* the decision is reached. For an
empty baseline this means `hash_before == fnv1a([])` and `hash_after == fnv1a([candidate])` — they
always differ — yet the outcome is marked Skipped and `would_inject = false`. The exposed `hash_*`
fields are then meaningless and look like a real diff to any consumer that reads them in isolation.
Either short-circuit before hashing or zero out the hash fields in the Skipped branch so they cannot
be mistaken for a true content delta.

- let (decision, would_inject, note) = if input.existing_lessons.is_empty() {
-         // 无现有 lesson 作为对比基准
-         (
-             ShadowDecision::Skipped,
-             false,
-             Some("no_baseline_lessons".into()),
-         )
-     } else if hash_before == hash_after {
+ if input.existing_lessons.is_empty() {
+         return ShadowOutcome {
+             change_id: input.change.change_id.clone(),
+             session_id: input.session_id.into(),
+             would_inject: None,
+             hash_before: String::new(),
+             hash_after: String::new(),
+             decision: ShadowDecision::Skipped,
+             note: Some("no_baseline_lessons".into()),
+             evaluated_at_ms: input.now_ms,
+         };
+     }
+     let (decision, would_inject, note) = if hash_before == hash_after {


─── src-tauri/src/evolution/sandbox/shadow.rs:95-97 ───
[bug · medium] `Vec::sort_by` is stable and the candidate is appended to the *end* of
`hypothetical`. This silently encodes the rule "ties go to established lessons": a candidate whose
importance equals any existing top-3 lesson will be sorted to the end of the equal-importance run
and therefore never enter the new top-3. Whether intentional or not, the rule is currently invisible
to callers — they only see a `Fail`/`below_top3_threshold` note and may mis-attribute the verdict to
a raw importance shortfall. Either document the rule next to the sort, or break ties by insertion
index so equal-importance candidates get a fair shake (or vice-versa).

+ // 注：Vec::sort_by 是 stable 的，candidate 被 push 到末尾，
+     // 因此 importance 与现有 top-3 lesson 相等时不会顶掉对方；
+     // 这是隐式的「平局归已建立 lesson」规则。
- let mut hypothetical = existing.clone();
+     let mut hypothetical = existing.clone();
      hypothetical.push(candidate.clone());
      hypothetical.sort_by(|a, b| b.importance.cmp(&a.importance));


─── src-tauri/src/evolution/sandbox/shadow.rs:101-107 ───
[bug · medium] When `existing_lessons` is empty the decision is *always* `Skipped` regardless of
candidate importance. In a cold-start scenario (no lessons have ever been written) every candidate
will Skipped and never Pass, so no change can ever advance past the shadow gate. This is a real
behavioral quirk of `run_shadow` that should be addressed at the policy layer (or here) — e.g., Pass
on empty baseline when candidate importance meets a minimum threshold, or document that callers must
seed lessons before shadow tests are meaningful.

+ // 冷启动场景：当前无 lesson baseline 时，重要性足够（>=4）
+ // 的 candidate 直接 Pass（否则全系统永远不会推进）。
  let (decision, would_inject, note) = if input.existing_lessons.is_empty() {
-         // 无现有 lesson 作为对比基准
-         (
-             ShadowDecision::Skipped,
-             false,
-             Some("no_baseline_lessons".into()),
-         )
+         if candidate.importance >= 4 {
+             (ShadowDecision::Pass, true, Some("cold_start_promote".into()))
+         } else {
+             (ShadowDecision::Skipped, false, Some("no_baseline_lessons".into()))
+         }
+     } else if hash_before == hash_after {


─── src-tauri/src/evolution/sandbox/shadow.rs:88-99 ───
[performance · low] `existing_lessons.to_vec()` is sorted, then `.clone()`d into `hypothetical` and
re-sorted. The first sort is used only to compute `hash_before`; we then pay for a full second clone
+ second sort on the very same data plus one element. A single pass that builds both before/after
top-3 indices (or a small fixed-size `BinaryHeap`-style selection) would cut the allocation and one
of the two O(n log n) sorts on every call. Low priority (small N today), but the pattern repeats and
is worth tightening before the caller is hit on a hot path.

- // 现有 lessons 按 importance 降序取 top-3
-     let mut existing = input.existing_lessons.to_vec();
-     existing.sort_by(|a, b| b.importance.cmp(&a.importance));
-     let before_top3: Vec<&ShadowLesson> = existing.iter().take(3).collect();
+ // 单遍选择 top-3：先取现有 top-3，再单独判断 candidate 是否进入。
+     let mut indexed: Vec<(usize, &ShadowLesson)> = input
+         .existing_lessons
+         .iter()
+         .enumerate()
+         .collect();
+     indexed.sort_by(|(_, x), (_, y)| y.importance.cmp(&x.importance));
+     let before_top3: Vec<&ShadowLesson> = indexed.iter().take(3).map(|(_, l)| *l).collect();
      let hash_before = hash_lessons_content(&before_top3);
  
-     // 加入 candidate，按 importance 模拟 top-3
-     let mut hypothetical = existing.clone();
-     hypothetical.push(candidate.clone());
-     hypothetical.sort_by(|a, b| b.importance.cmp(&a.importance));
-     let after_top3: Vec<&ShadowLesson> = hypothetical.iter().take(3).collect();
-     let hash_after = hash_lessons_content(&after_top3);
+     // 在 top-3 边界判断 candidate：仅在 candidate importance 严格大于
+     // before_top3 中最弱者时才进入，无需全量重排。
+     let last_in = before_top3.last().map(|l| l.importance).unwrap_or(i64::MIN);
+     let in_top3 = candidate.importance > last_in;
+     let hash_after = if in_top3 {
+         // 重建 after_top3 并 hash
+         ...
+     } else {
+         hash_before.clone()
+     };


─── src-tauri/src/evolution/sandbox/shadow.rs:373-383 ───
[test · low] The integration test asserts `sees_change` (count of canary=true out of
`session-0..session-999`) lies in `[25, 100]`. While `is_canary` is hash-based and deterministic,
FNV-1a mod 100 over sequential inputs `session-N` is *not* guaranteed to hit exactly 5% on any
specific slice; over 1000 trials it should land within a few σ (mean 50, σ ≈ 6.9), but the lower
bound 25 is ~3.6σ out — meaning a real regression that pushes canary distribution a few percent away
could silently mask or be masked. Either widen the bounds (e.g. `[15, 110]`), or use a fixed test
seed vector with a known exact hit count to make the assertion deterministic.

- // 6. Canary 路由验证：1000 sessions ≈ 5% 见到
-         let mut sees_change = 0;
-         for i in 0..1000 {
-             if is_canary(&format!("session-{i}")) {
-                 sees_change += 1;
-             }
-         }
+ // 6. Canary 路由验证：用固定种子，验收桶覆盖度（确定值，避免随机样本漂移）
+         let total = 1000usize;
+         let canary_count = (0..total).filter(|i| super::super::routing::is_canary(&format!("session-{i}"))).count();
+         let rate = canary_count as f64 / total as f64;
          assert!(
-             sees_change >= 25 && sees_change <= 100,
-             "Canary 5% 应 ≈ 50，实测 {sees_change}"
+             (0.02..=0.08).contains(&rate),
+             "Canary 命中率应在 2%–8% 之间，实测 {rate:.3} ({canary_count}/{total})"
          );


─── src-tauri/src/evolution/trace.rs:38-39 ───
[security · medium] The module-level privacy discipline requires only structured summaries (counts +
classification tags), and `error_kind` is explicitly scoped to categories like "timeout". But
`skill_used` stores the raw skill name with no documented allowlist or category rule. If skill names
can carry user-identifying context (user-named skills tied to a project, persona, or task), this
leaks PII into the audit stream. Either constrain the field to a known skill enum / allowlist, or
apply the same category-only rule used for `error_kind`. `task_refs: Vec<String>` has the same shape
of concern (the comment only rules out titles, not whether the ID itself is identifying).

- /// 命中的技能名（若有）。
+ /// 命中的技能分类标签（如 `"code_review"` / `"data_analysis"`），不存原始技能名。
      pub skill_used: Option<String>,


─── src-tauri/src/evolution/trace.rs:185-187 ───
[bug · medium] The comment claims clamping to 0 keeps the sampling rule "stable", but the same
`duration_ms > DURATION_THRESHOLD_MS` predicate still fails for 0, so traces whose actual elapsed
time exceeded the threshold but happened to straddle a wall-clock regression (NTP step-back, manual
clock change) are silently dropped from audit. Measure elapsed time via a monotonic clock
(`std::time::Instant` or `quanta`), and derive the absolute `ended_at_ms = started_at_ms +
elapsed_ms` so the sampling predicate stays correct across clock jumps.

- // 饱和减法：NTP 回拨/手动改时会让墙钟差为负——负值会把正常长轨迹
-     // 从「>60s」采样规则里静默漏掉，钳到 0 保采样口径稳定
-     let duration_ms = ended_at_ms.saturating_sub(ctx.started_at_ms);
+ // 用 monotonic 时钟算 elapsed，避免 NTP 回拨 / 改时把长轨迹静默漏掉
+     let duration_ms = started_instant.elapsed().as_millis() as i64;


─── src-tauri/src/evolution/trace.rs:188-188 ───
[bug · low] `tool_calls.len() as u32` is a narrowing cast. On 64-bit platforms, a `Vec` length can
in principle exceed `u32::MAX` and silently truncate, which can flip an over-threshold case into a
non-record and drop the audit signal. Change the API parameter to `usize` (matches `Vec::len` and
`tool_calls.len()` in the call sites) and drop the cast.

- if !should_record_trace(&ctx.outcome, ctx.tool_calls.len() as u32, duration_ms) {
+ if !should_record_trace(&ctx.outcome, ctx.tool_calls.len(), duration_ms) {


─── src-tauri/src/evolution/trace.rs:81-85 ───
[performance · low] `out.push_str(&format!("{b:02x}"))` allocates a fresh `String` for every byte
(16 throwaway allocations per recorded trace). Use `write!` against the pre-sized `String`, or
hex-encode into a `[u8; 16]` once and `format!` a single `String`. Cheap, but it's a clean win on
the helper that runs for every sampled trace.

- // 取 8 字节 -> 16 hex 字符（短 hash，便于 audit grep）
+ use std::fmt::Write as _;
      let mut out = String::with_capacity(16);
      for b in bytes {
-         out.push_str(&format!("{b:02x}"));
+         let _ = write!(out, "{b:02x}");
      }


─── src-tauri/src/evolution/trace.rs:73-74 ───
[maintainability · low] Note: `DefaultHasher::new()` in current Rust does use fixed keys (0, 0), so
the function IS deterministic across runs within a single Rust version (the pre-scan's
"non-deterministic across processes" framing is incorrect). The real caveat is that stdlib
explicitly states the algorithm "should not be relied upon over releases" — a future Rust upgrade
can change `DefaultHasher`'s algorithm and break long-term trace_id dedup against historical audit
data. Two practical fixes if you care about long-term stability: (1) swap to a stable hash (FNV-1a,
xxhash, blake3) so the ID format is decoupled from stdlib; (2) widen the output past 64 bits — 16
hex chars give ~4B-trace birthday bound, which is fine for a single user but may bite if audit logs
are aggregated across many users/machines.



─── src-tauri/src/evolution/trace.rs:220-221 ───
[maintainability · low] Two issues in one audit emission: (a) `unwrap_or_else(|_| "unknown")` falls
back to a literal that's inconsistent with the real `trace.outcome` and will silently break
downstream audit parsers if serialization ever fails (currently theoretical since the enum is
derive-only). Either `expect("outcome serialization")` (derive cannot fail), or propagate the error;
do not silently substitute a different value. (b) Emitting both `outcome` (JSON) and a flat
`aborted` boolean is redundant — `aborted` is trivially derivable from `outcome`, and the module's
own comment notes aborted is "唯一事实源 = outcome 枚举" with no independent flag. Drop the boolean.

  "outcome" => serde_json::to_string(&trace.outcome)
-             .unwrap_or_else(|_| String::from("unknown")),
+             .expect("TraceOutcome 序列化（derive 不会失败）"),



──────── Project Summary ────────

The user wants a project-level summary of a full-repo scan. I need to analyze the 134 comments across 29 files and produce a markdown document with specific sections.

Let me identify the patterns:

1. **Top Issues** - Most consequential findings, ranked by impact:
   - Concurrency/race conditions (TOCTOU, check-then-act, mutex poison recovery, missing fsync)
   - Error handling (flattened Result<String>, opaque errors)
   - PII/privacy leakage (raw content passed verbatim)
   - File I/O race conditions and cross-process races
   - Validation gaps (now_ms, window_days, etc.)
   - Type safety issues (bare bool, magic strings)
   - Dead code/deprecated APIs
   - Hardcoded paths (Windows incompatibility)
   - Determinism issues (chrono::Utc::now in pure functions)

2. **Module Hotspots** - Highest comment density:
   - src-tauri/src/evolution/observe/ - many metrics issues
   - src-tauri/src/evolution/sandbox/ - kill_switch, shadow, io issues
   - src-tauri/src/evolution/ - mod.rs, candidate/, derive.rs, emit.rs, apply.rs
   - src-tauri/src/evolution/panel/ - commands.rs

3. **Cross-Cutting Concerns**:
   - Mutex poison recovery via `into_inner()` is repeated
   - `Result<_, String>` flattening
   - `chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)` pattern
   - Hardcoded `/tmp/` paths
   - Documented invariants not enforced at API boundary
   - Deprecated APIs still in use
   - PII leakage in derive.rs (multiple branches)

4. **Quick Wins**:
   - Use `write!` instead of `format!` in trace.rs
   - Hoist temp-dir suffix to use process id + atomic counter
   - Add validation for now_ms
   - Remove dead `_unused_silence`
   - Pin FNV-1a test value

Let me organize this properly.

# Repo Scan Summary

## Top Issues

1. **Cross-process / cross-task write races in JSONL stores** (severity: high, impact: data corruption). Several "atomic append" functions (`evolution/candidate/entry.rs::append`, `evolution/change/record.rs::append`) document themselves as persistent but actually do separate `create_dir_all` → `open(O_APPEND)` → `write_all` with no `flock`, no `sync_all`, and no in-process store lock guarding the read-modify-write path (`evolution/candidate/mod.rs::write_proposals` race, `evolution/change/derive.rs::unique_change_id_for` TOCTOU). Torn lines and missing `change_id` uniqueness are documented outcomes.

2. **PII / raw content leakage in `evolution/derive.rs`** (severity: high, impact: privacy violation across three call sites). `suggestion.text` is built by concatenating the raw op `content` verbatim — only length-truncated — while the `summary` is sanitized. The fixture itself uses `/Users/renshi/secret/file.txt contains PII` as content, demonstrating the leak. Affects Merge and the other branch derivations.

3. **Lock-scope and lock-order violations in `evolution/mod.rs::post_consolidation`** (severity: high, impact: split-brain between proposals JSONL and `mem_items`). The comment claims `lock_evolution_store()` covers the apply step but it is released before `apply::apply_from_consolidation` runs; concurrent panel toggles / shadow / apply CR can interleave; `apply_from_consolidation`'s return value is not even inspected. Cascade path in `panel/commands.rs` further extends `EVOLUTION_STORE_LOCK` across `open_db` + `DB_WRITE_LOCK` acquisition, growing the critical section.

4. **Opaque `Result<_, String>` flattening across the evolution crate** (severity: medium-high, repetitive). At least 8 sites (`candidate/mod.rs`, `candidate/entry.rs`, `change/status.rs`, `change/record.rs`, `sandbox/kill_switch.rs`, `observe/stop.rs`, `policy.rs`, `mod.rs`) collapse `io::Error`/`serde_json::Error`/domain-specific outcomes into a single string. Callers cannot programmatically distinguish "missing", "malformed", "terminal-state", or "same-state". Audit/metrics code falls back to `e.contains("终态")`-style parsing.

5. **`chrono::Utc::now()` inside documented pure functions** (`derive.rs`, `emit.rs`, `sandbox/io.rs`, `sandbox/kill_switch.rs`). Derive docs claim pure-function contract; `created_at_ms` is non-deterministic across calls, the dedup-window timestamp underflows on NTP rollback (`now_ms.saturating_sub(*ts)`), and the temp-dir suffix `timestamp_nanos_opt().unwrap_or(0)` collapses to constant `0` on overflow — parallel `cargo test` runs collide on shared `std::env::temp_dir()`.

6. **Silent mutex-poison recovery via `e.into_inner()`** (4+ sites: `panel/commands.rs` ×3, `policy.rs`, `emit.rs`). After a panic mid-mutation, the protected state (jsonl file / SQLite rows / dedup map) is potentially half-applied, and code clears the poison and continues. Compounds any earlier corruption rather than failing closed.

7. **`evolution/sandbox/kill_switch.rs::load_from_file` silently rewrites in-memory state** when `all_auto_apply=true + shadow_only=false` (flips `shadow_only` to true without writing back). The doc-comment acknowledges the override but `eprintln!` is the only signal — no error type, no persistence.

8. **Status-machine `Approved → Active` transition has no policy gate** (`change/status.rs`). Doc admits hard-constraint ② bypass; `can_transition`/`transition` allow the edge purely on topology, so any caller can wire `transition` directly and skip the canary.

9. **Hardcoded `/tmp/...` paths in `evolution/sandbox/io.rs`** break on Windows and target a world-writable POSIX dir; combined with weak temp-dir suffixes this produces cross-test collisions.

10. **`evolution/observe/shadow.rs::shadow_apply_for_batch_with_app` — the production entry — has zero direct test coverage**, while every other path is exercised. Field-name mismatches between audit and code go undetected because the production branch cannot be reached from tests (no `AppHandle<Wry>`).

## Module Hotspots

- **`src-tauri/src/evolution/observe/`** — heaviest comment density. `metrics.rs`, `synthetic.rs`, `shadow.rs`, `stop.rs`, `mod.rs` together account for ~30 findings: window-boundary bugs, biased LCG, cold-start shadow Skipped, O(n²) loops, missing `now_ms` validation, fragile `await_holding_lock` allow-attr.
- **`src-tauri/src/evolution/sandbox/`** — `kill_switch.rs`, `shadow.rs`, `io.rs`, `routing.rs` cover mutex poison, file size cap, hardcoded paths, opaque errors, would_inject bool collapse, dead `_unused_silence` helper, allocation hotspots.
- **`src-tauri/src/evolution/panel/commands.rs`** — 9 distinct findings covering lock-order, self-transition hazard in `mark_human_applied`, redundant apply-on-Already-Active, three mutex-poison sites, and cascade critical-section growth.
- **`src-tauri/src/evolution/`** top-level `mod.rs`, `derive.rs`, `emit.rs`, `apply.rs`, `proposal.rs` — concurrency, PII, dedup, audit-availability, deprecated-API leakage, and trait-boundary violations.
- **`src-tauri/src/evolution/candidate/`** — `entry.rs`, `derive.rs`, `conflict.rs`, `mapping.rs` cross-module. Race conditions, redundant proposal-id checks, `Expired → Pending` wildcard, dependency on `target.tag()` being lossless, body-length invariant violations, `mk_proposal` always building `MemoryPolicy` regardless of category.
- **`src-tauri/src/evolution/change/`** — `record.rs`, `derive.rs`, `status.rs` — fsync omission, no change_id uniqueness enforcement, "pending" namespace collision, `Approved → Active` policy gap, TOCTOU in `unique_change_id_for`.
- **`tests-audit/audit_evolution_layering.py`** — 4 separate findings: Rust-lifetime/char-literal confusion, single-line `#[cfg(test)]` not matched, unanchored regexes fire on suffixes, selftest fixtures never exercise the fragile paths they guard.

## Cross-Cutting Concerns

- **`Result<_, String>` flattening** — pervasive across `candidate/{mod,entry}.rs`, `change/{status,record}.rs`, `sandbox/kill_switch.rs`, `observe/stop.rs`, `policy.rs`, `mod.rs`. Concrete file evidence in each.
- **Mutex-poison recovery via `into_inner()`** — `panel/commands.rs` (3 sites), `policy.rs`, `emit.rs`. Same pattern, same risk profile; should be a single project-wide helper or explicitly forbidden.
- **Doc-comment invariants not enforced at the API boundary** — `evolution/derive.rs` "pure function", `evolution/candidate/conflict.rs` "one-line delegation", `evolution/candidate/entry.rs` "caller-must-hold-lock", `evolution/change/status.rs` "canary policy gate", `evolution/observe/mod.rs` "no DB writes / no LLM". All are honored by convention only.
- **`chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)`** — appears in `sandbox/io.rs`, `sandbox/kill_switch.rs`, `observe/synthetic.rs`, and is the same weak uniqueness source in three different modules.
- **Deprecated APIs still wired into production** — `evolution/candidate/conflict.rs` tests + `evolution/change/derive.rs::from_proposal` both call `#[deprecated]` `passes_auto_apply_gate` / `layer_priority` / `impact_ord` / `resolve_conflict` / `sort_entries_cross_layer`. Under `-D warnings` this breaks CI.
- **O(n²) batch scans** — `evolution/apply.rs::apply_one` scans full store twice per proposal; `evolution/observe/synthetic.rs` linear-finds proposal_id inside the promoted loop; `evolution/change/derive.rs::unique_change_id_for` re-scans rows inside a 2..=9999 collision loop.
- **Test fixtures cover the wrong paths** — `audit_evolution_layering.py` selftest only samples ASCII identifier keywords, hiding the lifetime/char-literal and inline-`#[cfg(test)]` bugs. `shadow.rs` has no direct coverage of its only production entry point.
- **Validation gaps** — `now_ms`, `window_days`, `created_at_ms`, `proposal_id` character set, `KillSwitch` config consistency are all under-validated in different files.
- **`Pending` namespace collision** — `ChangeStatus::Pending` and `ApprovalSource::Pending` both serialize to `"pending"`, complicating log grep / key=value parsing (`change/record.rs`).

## Quick Wins

1. **`evolution/trace.rs`** — replace `out.push_str(&format!("{b:02x}"))` per byte with a single `write!` against the pre-sized `String`, or hex-encode into a fixed buffer. Drop 16 allocations per recorded trace.
2. **`evolution/sandbox/io.rs`** and **`evolution/sandbox/kill_switch.rs`** — replace `timestamp_nanos_opt().unwrap_or(0)` with a uniqueness source that mixes `std::process::id()` + an `AtomicU64` counter. Eliminates parallel-`cargo test` collisions and the `0`-suffix constant.
3. **`evolution/sandbox/io.rs`** — build temp paths from `std::env::temp_dir().join("evolution-…")` instead of hardcoded `/tmp/...`. Cross-platform fix.
4. **`evolution/sandbox/io.rs`** — delete `_unused_silence` and `mk_change` (or actually use them); the comment currently hides a real dead helper.
5. **`evolution/sandbox/routing.rs`** — pin the FNV-1a output to the exact bytes of `fnv1a("test")` (not just `assert_eq!(h, fnv1a("test"))`). Currently a self-equal assertion; a constant would lock the value.
6. **`evolution/sandbox/routing.rs`** — hoist the four `format!("session-{i}")` calls per loop iteration in the orthogonality test into a single `let`.
7. **`evolution/observe/synthetic.rs`** — replace the LCG-based `next_int` with the same hash-based deterministic draws used elsewhere, or pre-validate that `window_days` covers the entire proposal population. Removes modulo-bias in `pick_layer`.
8. **`evolution/observe/metrics.rs`** — flip `<= now_ms` to `< now_ms` in `candidate_generation_rate` to match the documented `>= start && < now` window. One-character semantic fix.
9. **`evolution/observe/metrics.rs`** — propagate `window_days <= 0` instead of silently coercing to `1.0` (return `Result`, or assert and log).
10. **`evolution/observe/synthetic.rs`** — validate `now_ms >= cfg.window_days * MS_PER_DAY` in `validate()`; one extra check eliminates a debug-panic / release-wrap overflow path.
11. **`evolution/observe/synthetic.rs`** — build a `HashMap<&str, &Proposal>` once outside the promoted loop; eliminates the `O(n·m)` linear `.find` and the `.unwrap_or` silent default.
12. **`evolution/candidate/entry.rs`** — replace hand-rolled `ProposalStatus::as_str` with a `#[serde(rename_all = "snake_case")]`-derived serializer so adding a variant cannot desynchronize wire format and accessor.
13. **`evolution/candidate/conflict.rs`** — delete the redundant `&& e.proposal_id != new.proposal_id` tail in `is_conflict` (already excluded upstream).
14. **`evolution/candidate/derive.rs`** — replace the five `.clone()`s in `from_proposal` with `into_owned`-style field moves when callers can pass `EvolutionProposal` by value.
15. **`evolution/trace.rs`** — change `tool_calls.len() as u32` to `tool_calls.len() as usize` (or accept `usize`) to avoid silent truncation on 64-bit platforms.
16. **`tests-audit/audit_evolution_layering.py`** — anchor every forbidden pattern with `\b` on the left, match `#[cfg(test)]\s*mod` regardless of newline, and add Rust-lifetime + inline-`#[cfg(test)]` samples to `SELFTEST_FAIL_SAMPLES` so the regression guard actually exercises the fragile paths.
