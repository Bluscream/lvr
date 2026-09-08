use std::time::{Duration, Instant};

use crate::config::{AutostartEntry, Trigger};
use crate::procs::ProcSnapshot;
use crate::wivrn::WivrnState;

use super::entries::{Action, PlanInput};
use super::*;

fn entry() -> AutostartEntry {
    AutostartEntry {
        id: "test".into(),
        name: "Test".into(),
        enabled: true,
        trigger: Trigger::Vrchat,
        command: "/bin/true".into(),
        grace_secs: 120,
        ..Default::default()
    }
}

fn input(trigger_active: bool, running: bool, now: Instant) -> PlanInput {
    PlanInput {
        trigger_active,
        running,
        now,
        relaunch_debounce: Duration::from_secs(30),
    }
}

#[test]
fn starts_when_trigger_appears() {
    let mut runtime = EntryRuntime::default();
    let now = Instant::now();
    assert_eq!(
        runtime.plan(&entry(), input(true, false, now)),
        Action::Start
    );
}

#[test]
fn does_not_start_twice_while_the_app_boots() {
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    assert_eq!(
        runtime.plan(&entry(), input(true, false, start)),
        Action::Start
    );
    // Still not visible in the process table a second later.
    assert_eq!(
        runtime.plan(&entry(), input(true, false, start + Duration::from_secs(1))),
        Action::None
    );
    // And not even after the debounce, because it already launched.
    assert_eq!(
        runtime.plan(
            &entry(),
            input(true, false, start + Duration::from_secs(60))
        ),
        Action::None
    );
}

#[test]
fn restart_on_exit_relaunches_after_the_debounce() {
    let config = AutostartEntry {
        restart_on_exit: true,
        ..entry()
    };
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    assert_eq!(
        runtime.plan(&config, input(true, false, start)),
        Action::Start
    );
    assert_eq!(
        runtime.plan(&config, input(true, false, start + Duration::from_secs(5))),
        Action::None
    );
    assert_eq!(
        runtime.plan(&config, input(true, false, start + Duration::from_secs(31))),
        Action::Start
    );
}

#[test]
fn stops_only_after_the_grace_period() {
    let config = entry();
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    runtime.plan(&config, input(true, true, start));

    assert_eq!(
        runtime.plan(&config, input(false, true, start + Duration::from_secs(1))),
        Action::None
    );
    assert_eq!(
        runtime.plan(
            &config,
            input(false, true, start + Duration::from_secs(119))
        ),
        Action::None
    );
    assert_eq!(
        runtime.plan(
            &config,
            input(false, true, start + Duration::from_secs(121))
        ),
        Action::Stop
    );
}

#[test]
fn an_app_that_was_already_running_at_startup_is_left_alone() {
    let config = entry();
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    for minutes in [0, 5, 60] {
        assert_eq!(
            runtime.plan(
                &config,
                input(false, true, start + Duration::from_secs(minutes * 60))
            ),
            Action::None
        );
    }
    assert!(runtime.stop_at.is_none());

    runtime.plan(
        &config,
        input(true, true, start + Duration::from_secs(3600)),
    );
    assert!(runtime.armed);
    runtime.plan(
        &config,
        input(false, true, start + Duration::from_secs(3601)),
    );
    assert_eq!(
        runtime.plan(
            &config,
            input(false, true, start + Duration::from_secs(3800))
        ),
        Action::Stop
    );
}

#[test]
fn grace_of_zero_stops_immediately() {
    let config = AutostartEntry {
        grace_secs: 0,
        ..entry()
    };
    let mut runtime = EntryRuntime::default();
    let now = Instant::now();
    runtime.plan(&config, input(true, true, now));
    assert_eq!(runtime.plan(&config, input(false, true, now)), Action::Stop);
}

#[test]
fn negative_grace_never_stops() {
    let config = AutostartEntry {
        grace_secs: -1,
        ..entry()
    };
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    runtime.plan(&config, input(true, true, start));
    for minutes in [1, 10, 600] {
        assert_eq!(
            runtime.plan(
                &config,
                input(false, true, start + Duration::from_secs(minutes * 60))
            ),
            Action::None
        );
    }
    assert_eq!(runtime.stop_at, None);
}

#[test]
fn trigger_returning_cancels_a_pending_stop() {
    let config = entry();
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    runtime.plan(&config, input(true, true, start));
    runtime.plan(&config, input(false, true, start + Duration::from_secs(1)));
    assert!(runtime.stop_at.is_some());
    runtime.plan(&config, input(true, true, start + Duration::from_secs(2)));
    assert!(runtime.stop_at.is_none());
    assert_eq!(
        runtime.plan(&config, input(false, true, start + Duration::from_secs(3))),
        Action::None
    );
}

#[test]
fn start_delay_is_honoured() {
    let config = AutostartEntry {
        start_delay_secs: 10,
        ..entry()
    };
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    assert_eq!(
        runtime.plan(&config, input(true, false, start)),
        Action::None
    );
    assert_eq!(
        runtime.plan(&config, input(true, false, start + Duration::from_secs(5))),
        Action::None
    );
    assert_eq!(
        runtime.plan(&config, input(true, false, start + Duration::from_secs(11))),
        Action::Start
    );
}

#[test]
fn disabled_entries_do_nothing() {
    let config = AutostartEntry {
        enabled: false,
        ..entry()
    };
    let mut runtime = EntryRuntime::default();
    let now = Instant::now();
    assert_eq!(runtime.plan(&config, input(true, false, now)), Action::None);
    assert_eq!(runtime.plan(&config, input(false, true, now)), Action::None);
}

#[test]
fn suppression_blocks_restart_until_the_trigger_cycles() {
    let config = entry();
    let mut runtime = EntryRuntime::default();
    let start = Instant::now();
    runtime.suppressed = true;
    assert_eq!(
        runtime.plan(&config, input(true, false, start)),
        Action::None
    );

    runtime.plan(&config, input(false, false, start + Duration::from_secs(1)));
    assert!(!runtime.suppressed);
    assert_eq!(
        runtime.plan(&config, input(true, false, start + Duration::from_secs(2))),
        Action::Start
    );
}

#[test]
fn manual_trigger_is_never_active() {
    let snapshot = ProcSnapshot::default();
    let wivrn = WivrnState {
        running: true,
        headset_connected: true,
        ..Default::default()
    };
    assert!(!Engine::trigger_active(
        &Trigger::Manual,
        &snapshot,
        &wivrn,
        true
    ));
}

#[test]
fn trigger_active_maps_each_source() {
    let snapshot = ProcSnapshot {
        procs: vec![crate::procs::ProcInfo {
            pid: 5,
            ppid: None,
            haystack: "some-daemon --run".into(),
        }],
    };
    let off = WivrnState::default();
    let on = WivrnState {
        running: true,
        headset_connected: true,
        ..Default::default()
    };

    assert!(Engine::trigger_active(
        &Trigger::Vrchat,
        &snapshot,
        &off,
        true
    ));
    assert!(!Engine::trigger_active(
        &Trigger::Vrchat,
        &snapshot,
        &off,
        false
    ));
    assert!(Engine::trigger_active(
        &Trigger::WivrnRunning,
        &snapshot,
        &on,
        false
    ));
    assert!(!Engine::trigger_active(
        &Trigger::WivrnRunning,
        &snapshot,
        &off,
        false
    ));
    assert!(Engine::trigger_active(
        &Trigger::HeadsetConnected,
        &snapshot,
        &on,
        false
    ));
    assert!(Engine::trigger_active(
        &Trigger::Process("SOME-daemon".into()),
        &snapshot,
        &off,
        false
    ));
    assert!(!Engine::trigger_active(
        &Trigger::Process("  ".into()),
        &snapshot,
        &off,
        false
    ));
}

#[test]
fn seconds_until_counts_down_and_never_goes_negative() {
    let now = Instant::now();
    assert_eq!(EntryRuntime::seconds_until(None, now), None);
    assert_eq!(
        EntryRuntime::seconds_until(Some(now + Duration::from_secs(42)), now),
        Some(42)
    );
    assert_eq!(
        EntryRuntime::seconds_until(Some(now - Duration::from_secs(42)), now),
        Some(0)
    );
}

#[test]
fn manual_start_long_after_vrchat_closed_is_left_alone() {
    let entry = entry();
    let mut runtime = EntryRuntime::default();
    let t0 = Instant::now();

    runtime.plan(&entry, input(true, true, t0));
    runtime.plan(&entry, input(false, true, t0));
    let after_grace = t0 + Duration::from_secs(121);
    assert_eq!(runtime.plan(&entry, input(false, true, after_grace)), Action::Stop);

    runtime.plan(&entry, input(false, false, after_grace));

    let much_later = after_grace + Duration::from_secs(10_000);
    assert_eq!(runtime.plan(&entry, input(false, true, much_later)), Action::None);
    assert_eq!(runtime.stop_at, None);

    let later_still = much_later + Duration::from_secs(600);
    assert_eq!(runtime.plan(&entry, input(false, true, later_still)), Action::None);
}

#[test]
fn a_new_vrchat_session_rearms_the_grace_period() {
    let entry = entry();
    let mut runtime = EntryRuntime::default();
    let t0 = Instant::now();

    runtime.plan(&entry, input(true, true, t0));
    runtime.plan(&entry, input(false, true, t0));
    let after_grace = t0 + Duration::from_secs(121);
    assert_eq!(runtime.plan(&entry, input(false, true, after_grace)), Action::Stop);
    runtime.plan(&entry, input(false, false, after_grace));

    let t1 = after_grace + Duration::from_secs(3_600);
    runtime.plan(&entry, input(true, true, t1));
    assert_eq!(runtime.plan(&entry, input(false, true, t1)), Action::None);
    assert_eq!(
        runtime.plan(&entry, input(false, true, t1 + Duration::from_secs(121))),
        Action::Stop
    );
}

#[test]
fn an_entry_that_exits_on_its_own_during_grace_does_not_stay_armed() {
    let entry = entry();
    let mut runtime = EntryRuntime::default();
    let t0 = Instant::now();

    runtime.plan(&entry, input(true, true, t0));
    runtime.plan(&entry, input(false, true, t0));
    assert!(runtime.stop_at.is_some());

    runtime.plan(&entry, input(false, false, t0 + Duration::from_secs(5)));
    assert_eq!(runtime.stop_at, None);

    assert_eq!(
        runtime.plan(&entry, input(false, true, t0 + Duration::from_secs(10))),
        Action::None
    );
    assert_eq!(runtime.stop_at, None);
}

#[test]
fn a_zero_grace_stop_also_disarms() {
    let mut entry = entry();
    entry.grace_secs = 0;
    let mut runtime = EntryRuntime::default();
    let t0 = Instant::now();

    runtime.plan(&entry, input(true, true, t0));
    assert_eq!(runtime.plan(&entry, input(false, true, t0)), Action::Stop);
    runtime.plan(&entry, input(false, false, t0));

    assert_eq!(
        runtime.plan(&entry, input(false, true, t0 + Duration::from_secs(60))),
        Action::None
    );
}
