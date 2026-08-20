//! The #175 adapter contract matrix.
//!
//! Ten view-level contracts, written once in `src/contract/contracts.rs`,
//! replayed here against three rigs: the scripted mock, WsNative against a
//! REAL spawned `jeliyad`, and DirectClient against a real in-process
//! Engine. Every contract×rig cell that does not run carries a recorded
//! reason — a gap is never silent — and the meta-test polices the table's
//! own honesty: every contract runs on at least one rig, every N/A carries
//! a non-empty reason, and all three rigs carry load.
//!
//! The suite is the permanent adapter gate for the v2 stack (clean-slate
//! cutover): a missing `jeliyad` binary or data dir FAILS the rig rather
//! than skipping, because a skipped oracle reads as green while covering
//! nothing.

mod rig_direct;
mod rig_mock;
mod rig_native;

use std::path::PathBuf;

use jeliya_client::contract::contracts;
use jeliya_client::contract::{PendingPlan, Rig, RigConfig};
use rig_direct::DirectRig;
use rig_mock::MockRig;
use rig_native::NativeRig;

// ---------------------------------------------------------------------------
// The matrix — the one table this suite is
// ---------------------------------------------------------------------------

/// One cell: the contract runs here — anchored to its actual test fn, so
/// the association is per-cell, not aggregate — or it is N/A for exactly
/// this reason.
enum Cell {
    /// The contract runs on this rig, via THIS test function. A `Runs`
    /// cell cannot name a test that does not exist (compile error) or a
    /// test another cell already anchors (the distinctness check in
    /// `matrix_cells_reference_real_tests`).
    Runs(fn(), &'static str),
    Na(&'static str),
}

/// Per-cell anchors: each wraps the one `#[tokio::test]` it tethers. The
/// `let _ = ...()` reference makes the matrix's claim and the test's
/// existence inseparable at compile time.
fn t_mock_c1() {
    let _ = mock_c1_lifecycle_reaches_ready;
}
fn t_mock_c2() {
    let _ = mock_c2_typed_calls_round_trip;
}
fn t_mock_c3() {
    let _ = mock_c3_pushes_fan_out_never_as_reply;
}
fn t_mock_c4() {
    let _ = mock_c4_queue_full_classification;
}
fn t_mock_c5u() {
    let _ = mock_c5_cancellation_classification;
}
fn t_mock_c5s() {
    let _ = mock_c5_sent_cancellation_classifies_unknown;
}
fn t_mock_c7() {
    let _ = mock_c7_ambiguous_execution_is_op_id_conflict;
}
fn t_mock_c10() {
    let _ = mock_c10_stop_settles_and_refuses;
}
fn t_native_c1() {
    let _ = native_c1_lifecycle_reaches_ready;
}
fn t_native_c2() {
    let _ = native_c2_typed_calls_round_trip;
}
fn t_native_c3() {
    let _ = native_c3_pushes_fan_out_never_as_reply;
}
fn t_native_c4() {
    let _ = native_c4_queue_bounds_are_local_not_wire;
}
fn t_native_c5() {
    let _ = native_c5_cancellation_classification;
}
fn t_native_c6() {
    let _ = native_c6_version_mismatch_is_terminal;
}
fn t_native_c7() {
    let _ = native_c7_ambiguous_execution_is_op_id_conflict;
}
fn t_native_c8() {
    let _ = native_c8_transport_loss_settles_honestly;
}
fn t_native_c9() {
    let _ = native_c9_restart_preserves_durable_state;
}
fn t_native_c10() {
    let _ = native_c10_stop_settles_and_refuses;
}
fn t_direct_c1() {
    let _ = direct_c1_lifecycle_reaches_ready;
}
fn t_direct_c2() {
    let _ = direct_c2_typed_calls_round_trip;
}
fn t_direct_c3() {
    let _ = direct_c3_pushes_fan_out_never_as_reply;
}
fn t_direct_c4() {
    let _ = direct_c4_queue_bounds_are_local_not_wire;
}
fn t_direct_c5() {
    let _ = direct_c5_cancellation_classification;
}
fn t_direct_c7() {
    let _ = direct_c7_ambiguous_execution_is_op_id_conflict;
}
fn t_direct_c9() {
    let _ = direct_c9_restart_preserves_durable_state;
}
fn t_direct_c10() {
    let _ = direct_c10_stop_settles_and_refuses;
}

const fn na(reason: &'static str) -> Cell {
    Cell::Na(reason)
}

/// Bind a `Runs` cell to its anchor: the fn (existence tether — a deleted
/// test is a compile error) and its name (deterministic distinctness —
/// two cells anchoring one test is a matrix bug). One token produces both,
/// so the pair cannot drift apart.
macro_rules! runs {
    ($f:ident) => {
        Cell::Runs($f, stringify!($f))
    };
}

/// (contract, mock, native, direct). Test functions below are generated from
/// exactly these rows — every `Runs` cell has a test, every `Na` cell does
/// not, and the meta-test polices the table's own honesty.
const MATRIX: &[(&str, Cell, Cell, Cell)] = &[
    (
        "c1 lifecycle reaches Ready",
        runs!(t_mock_c1),
        runs!(t_native_c1),
        runs!(t_direct_c1),
    ),
    (
        "c2 typed calls round-trip",
        runs!(t_mock_c2),
        runs!(t_native_c2),
        runs!(t_direct_c2),
    ),
    (
        "c3 pushes fan out, never as replies",
        runs!(t_mock_c3),
        runs!(t_native_c3),
        runs!(t_direct_c3),
    ),
    (
        "c4 queue bounds are local not wire (enforcement)",
        na("the mock has no kernel queue to overflow — its bounds are scripted; the classification leg (c4_queue_full_classification) runs here instead, and real enforcement is pinned by the native/direct kernels"),
        runs!(t_native_c4),
        runs!(t_direct_c4),
    ),
    (
        "c4 queue-full classification (scripted)",
        runs!(t_mock_c4),
        na("the native kernel enforces the real bound — that is the enforcement row; scripting a QueueFull here would test nothing the enforcement row does not"),
        na("the direct kernel enforces the real bound — see the enforcement row"),
    ),
    (
        "c5 cancellation classification (unsent)",
        runs!(t_mock_c5u),
        runs!(t_native_c5),
        runs!(t_direct_c5),
    ),
    (
        "c5 sent cancellation classifies Unknown",
        runs!(t_mock_c5s),
        na("on a real daemon the reply can land between cancellation and observation — honest behavior, not a breach; only a scripted oracle makes the sent class deterministic"),
        na("same as native: the engine's reply races the cancellation honestly"),
    ),
    (
        "c6 exact-version mismatch is terminal",
        na("the mock has no wire gate to refuse a version"),
        runs!(t_native_c6),
        na("in-process: there is no upgrade request and no version to refuse"),
    ),
    (
        "c7 ambiguous execution is op_id_conflict",
        runs!(t_mock_c7),
        runs!(t_native_c7),
        runs!(t_direct_c7),
    ),
    (
        "c8 transport loss settles honestly",
        na("a scripted backend has no transport or reconnect machinery; loss classification is pinned by the kernel fault suite (tests/kernel_fault.rs)"),
        runs!(t_native_c8),
        na("in-process engine: no socket exists to sever"),
    ),
    (
        "c9 restart preserves durable state",
        na("a scripted backend has no persistent state to preserve"),
        runs!(t_native_c9),
        runs!(t_direct_c9),
    ),
    (
        "c10 stop settles and refuses",
        runs!(t_mock_c10),
        runs!(t_native_c10),
        runs!(t_direct_c10),
    ),
];

fn rig_load(pick: fn(&'static (&'static str, Cell, Cell, Cell)) -> &'static Cell) -> usize {
    MATRIX
        .iter()
        .map(pick)
        .filter(|c| matches!(c, Cell::Runs(..)))
        .count()
}

/// Per-cell tether: every `Runs` cell anchors a REAL test fn (compile
/// error if it does not exist), and no two cells may anchor the same one —
/// so coverage claims are per-cell facts, not aggregate counts. This
/// closes the swap window where one cell flips to `Na` and another to
/// `Runs` naming a test that never ran.
#[test]
fn matrix_cells_reference_real_tests() {
    let anchored: Vec<(usize, &'static str, &'static str)> = MATRIX
        .iter()
        .enumerate()
        .flat_map(|(row, (_, mock, native, direct))| {
            [
                (mock, "mock", row),
                (native, "native", row),
                (direct, "direct", row),
            ]
        })
        .filter_map(|(cell, rig, row)| match cell {
            // Reading `f` here is the point of the field: the cell carries
            // the anchor so a deleted test fn is a COMPILE error, before
            // any test runs.
            Cell::Runs(f, name) => {
                let _: fn() = *f;
                Some((row, rig, *name))
            }
            Cell::Na(_) => None,
        })
        .collect();
    assert!(!anchored.is_empty(), "the matrix anchors no tests at all");
    for (i, (row_a, rig_a, name_a)) in anchored.iter().enumerate() {
        for (row_b, rig_b, name_b) in anchored[i + 1..].iter() {
            // Distinctness is over the NAME (deterministic), never the fn
            // address (fn-pointer comparison is documented-unreliable).
            // The name and the fn come from one `runs!` token, so they
            // cannot disagree; and the fn's existence is still enforced at
            // compile time by the cell itself.
            assert!(
                name_a != name_b,
                "two Runs cells (row {row_a}/{rig_a} and row {row_b}/{rig_b}) anchor the SAME test ({name_a}) — each Runs cell must name its own test"
            );
        }
    }
}

/// The suite's own honesty gate: no contract runs nowhere, no gap is
/// unjustified, and the parameterization is real (all rigs carry load).
#[test]
fn matrix_has_no_silent_gaps() {
    for (contract, mock, native, direct) in MATRIX {
        let cells = [mock, native, direct];
        let runs = cells.iter().filter(|c| matches!(c, Cell::Runs(..))).count();
        assert!(
            runs >= 1,
            "contract {contract:?} runs on NO rig — it must run somewhere or leave the suite"
        );
        for cell in cells {
            if let Cell::Na(reason) = cell {
                assert!(
                    !reason.trim().is_empty(),
                    "contract {contract:?} has an empty N/A reason — a gap must be justified"
                );
            }
        }
    }
    let loads = (
        rig_load(|row| &row.1),
        rig_load(|row| &row.2),
        rig_load(|row| &row.3),
    );
    assert!(
        loads.0 > 0 && loads.1 > 0 && loads.2 > 0,
        "expected all three rigs to carry contracts, got (mock={}, native={}, direct={})",
        loads.0,
        loads.1,
        loads.2
    );
}

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

fn base_config(label: &str) -> RigConfig {
    RigConfig::new(label, data_dir(label), jeliyad_bin())
}

fn data_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("jeliya-175-{label}-{}", std::process::id()))
}

fn jeliyad_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("JELIYAD_BIN") {
        return PathBuf::from(bin);
    }
    // tests/contract_suite lies three levels under the workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/jeliyad")
}

// ---------------------------------------------------------------------------
// Mock rig tests (classification legs + sent-class cancellation)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_c1_lifecycle_reaches_ready() {
    let mut rig = MockRig::spawn(base_config("mock-c1"))
        .await
        .expect("rig spawn");
    contracts::c1_lifecycle_reaches_ready(&mut rig)
        .await
        .expect("c1");
}

#[tokio::test]
async fn mock_c2_typed_calls_round_trip() {
    let mut rig = MockRig::spawn(base_config("mock-c2"))
        .await
        .expect("rig spawn");
    contracts::c2_typed_calls_round_trip(&mut rig)
        .await
        .expect("c2");
}

#[tokio::test]
async fn mock_c3_pushes_fan_out_never_as_reply() {
    let mut rig = MockRig::spawn(base_config("mock-c3"))
        .await
        .expect("rig spawn");
    contracts::c3_pushes_fan_out_never_as_reply(&mut rig)
        .await
        .expect("c3");
}

#[tokio::test]
async fn mock_c4_queue_full_classification() {
    let mut config = base_config("mock-c4");
    config.plan = PendingPlan::LocalQueueFull;
    let mut rig = MockRig::spawn(config).await.expect("rig spawn");
    rig_mock::c4_classification_mock(&mut rig)
        .await
        .expect("c4 classification");
}

#[tokio::test]
async fn mock_c5_cancellation_classification() {
    let mut config = base_config("mock-c5u");
    config.plan = PendingPlan::HangUnsent;
    let mut rig = MockRig::spawn(config).await.expect("rig spawn");
    contracts::c5_cancellation_classification(&mut rig)
        .await
        .expect("c5 unsent");
}

#[tokio::test]
async fn mock_c5_sent_cancellation_classifies_unknown() {
    let mut config = base_config("mock-c5s");
    config.plan = PendingPlan::HangSent;
    let mut rig = MockRig::spawn(config).await.expect("rig spawn");
    contracts::c5_sent_cancellation_classifies_unknown(&mut rig)
        .await
        .expect("c5 sent");
}

#[tokio::test]
async fn mock_c7_ambiguous_execution_is_op_id_conflict() {
    let mut rig = MockRig::spawn(base_config("mock-c7"))
        .await
        .expect("rig spawn");
    contracts::c7_ambiguous_execution_is_op_id_conflict(&mut rig)
        .await
        .expect("c7");
}

#[tokio::test]
async fn mock_c10_stop_settles_and_refuses() {
    let mut config = base_config("mock-c10");
    config.plan = PendingPlan::HangUnsent;
    let mut rig = MockRig::spawn(config).await.expect("rig spawn");
    contracts::c10_stop_settles_and_refuses(&mut rig)
        .await
        .expect("c10");
}

// ---------------------------------------------------------------------------
// Native rig tests (real spawned jeliyad)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_c1_lifecycle_reaches_ready() {
    let mut rig = NativeRig::spawn(base_config("native-c1"))
        .await
        .expect("rig spawn");
    contracts::c1_lifecycle_reaches_ready(&mut rig)
        .await
        .expect("c1");
}

#[tokio::test]
async fn native_c2_typed_calls_round_trip() {
    let mut rig = NativeRig::spawn(base_config("native-c2"))
        .await
        .expect("rig spawn");
    contracts::c2_typed_calls_round_trip(&mut rig)
        .await
        .expect("c2");
}

#[tokio::test]
async fn native_c3_pushes_fan_out_never_as_reply() {
    let mut rig = NativeRig::spawn(base_config("native-c3"))
        .await
        .expect("rig spawn");
    contracts::c3_pushes_fan_out_never_as_reply(&mut rig)
        .await
        .expect("c3");
}

#[tokio::test]
async fn native_c4_queue_bounds_are_local_not_wire() {
    let mut rig = NativeRig::spawn(base_config("native-c4").nothing_ever_sends())
        .await
        .expect("rig spawn");
    contracts::c4_queue_bounds_are_local_not_wire(&mut rig)
        .await
        .expect("c4");
}

#[tokio::test]
async fn native_c5_cancellation_classification() {
    let mut rig = NativeRig::spawn(base_config("native-c5").nothing_ever_sends())
        .await
        .expect("rig spawn");
    contracts::c5_cancellation_classification(&mut rig)
        .await
        .expect("c5");
}

#[tokio::test]
async fn native_c6_version_mismatch_is_terminal() {
    let mut config = base_config("native-c6");
    config.gate_version = 3; // above PROTOCOL: the daemon refuses it at the gate
    let mut rig = NativeRig::spawn(config).await.expect("rig spawn");
    contracts::c6_version_mismatch_is_terminal(&mut rig)
        .await
        .expect("c6");
}

#[tokio::test]
async fn native_c7_ambiguous_execution_is_op_id_conflict() {
    let mut rig = NativeRig::spawn(base_config("native-c7"))
        .await
        .expect("rig spawn");
    contracts::c7_ambiguous_execution_is_op_id_conflict(&mut rig)
        .await
        .expect("c7");
}

#[tokio::test]
async fn native_c8_transport_loss_settles_honestly() {
    let mut rig = NativeRig::spawn(
        base_config("native-c8")
            .nothing_ever_sends()
            .fail_fast_reconnect(),
    )
    .await
    .expect("rig spawn");
    contracts::c8_transport_loss_settles_honestly(&mut rig)
        .await
        .expect("c8");
}

#[tokio::test]
async fn native_c9_restart_preserves_durable_state() {
    let mut rig = NativeRig::spawn(base_config("native-c9").patient_reconnect())
        .await
        .expect("rig spawn");
    contracts::c9_restart_preserves_durable_state(&mut rig)
        .await
        .expect("c9");
}

#[tokio::test]
async fn native_c10_stop_settles_and_refuses() {
    let mut rig = NativeRig::spawn(base_config("native-c10").nothing_ever_sends())
        .await
        .expect("rig spawn");
    contracts::c10_stop_settles_and_refuses(&mut rig)
        .await
        .expect("c10");
}

// ---------------------------------------------------------------------------
// Direct rig tests (real in-process Engine)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_c1_lifecycle_reaches_ready() {
    let mut rig = DirectRig::spawn(base_config("direct-c1"))
        .await
        .expect("rig spawn");
    contracts::c1_lifecycle_reaches_ready(&mut rig)
        .await
        .expect("c1");
}

#[tokio::test]
async fn direct_c2_typed_calls_round_trip() {
    let mut rig = DirectRig::spawn(base_config("direct-c2"))
        .await
        .expect("rig spawn");
    contracts::c2_typed_calls_round_trip(&mut rig)
        .await
        .expect("c2");
}

#[tokio::test]
async fn direct_c3_pushes_fan_out_never_as_reply() {
    let mut rig = DirectRig::spawn(base_config("direct-c3"))
        .await
        .expect("rig spawn");
    contracts::c3_pushes_fan_out_never_as_reply(&mut rig)
        .await
        .expect("c3");
}

#[tokio::test]
async fn direct_c4_queue_bounds_are_local_not_wire() {
    let mut rig = DirectRig::spawn(base_config("direct-c4").nothing_ever_sends())
        .await
        .expect("rig spawn");
    contracts::c4_queue_bounds_are_local_not_wire(&mut rig)
        .await
        .expect("c4");
}

#[tokio::test]
async fn direct_c5_cancellation_classification() {
    let mut rig = DirectRig::spawn(base_config("direct-c5").nothing_ever_sends())
        .await
        .expect("rig spawn");
    contracts::c5_cancellation_classification(&mut rig)
        .await
        .expect("c5");
}

#[tokio::test]
async fn direct_c7_ambiguous_execution_is_op_id_conflict() {
    let mut rig = DirectRig::spawn(base_config("direct-c7"))
        .await
        .expect("rig spawn");
    contracts::c7_ambiguous_execution_is_op_id_conflict(&mut rig)
        .await
        .expect("c7");
}

#[tokio::test]
async fn direct_c9_restart_preserves_durable_state() {
    let mut rig = DirectRig::spawn(base_config("direct-c9"))
        .await
        .expect("rig spawn");
    contracts::c9_restart_preserves_durable_state(&mut rig)
        .await
        .expect("c9");
}

#[tokio::test]
async fn direct_c10_stop_settles_and_refuses() {
    let mut rig = DirectRig::spawn(base_config("direct-c10").nothing_ever_sends())
        .await
        .expect("rig spawn");
    contracts::c10_stop_settles_and_refuses(&mut rig)
        .await
        .expect("c10");
}
