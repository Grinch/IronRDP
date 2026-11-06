//! Audio tap tests for `ironrdp-rdpsnd-native` (crate has `test = false`).

use core::time::Duration;

use ironrdp_rdpsnd_native::tap::{AUDIBLE, AudioTap, MAX_QUEUED};

#[test]
fn disabled_tap_queues_nothing() {
    let tap = AudioTap::new();
    tap.push(48_000, 2, &[1, 2, 3, 4]);
    assert!(tap.pop().is_none());
}

#[test]
fn enabled_tap_keeps_order_and_layout() {
    let tap = AudioTap::new();
    tap.set_enabled(true);
    tap.push(48_000, 2, &[1, 2]);
    tap.push(22_050, 1, &[3]);
    let first = tap.pop().expect("first chunk");
    assert_eq!(
        (first.sample_rate, first.channels, first.samples),
        (48_000, 2, vec![1, 2])
    );
    let second = tap.pop().expect("second chunk");
    assert_eq!(
        (second.sample_rate, second.channels, second.samples),
        (22_050, 1, vec![3])
    );
    assert!(tap.pop().is_none());
}

#[test]
fn clones_share_the_queue() {
    let tap = AudioTap::new();
    let backend_side = tap.clone();
    tap.set_enabled(true);
    backend_side.push(8_000, 1, &[7]);
    assert_eq!(tap.pop().expect("chunk").samples, vec![7]);
}

#[test]
fn full_queue_drops_oldest() {
    let tap = AudioTap::new();
    tap.set_enabled(true);
    // One second of mono 8 kHz per chunk, with the chunk number as its samples.
    let seconds = MAX_QUEUED.as_secs() + 3;
    for n in 0..seconds {
        tap.push(8_000, 1, &vec![i16::try_from(n).unwrap(); 8_000]);
    }
    assert_eq!(tap.dropped(), 3);
    let first = tap.pop().expect("chunk");
    assert_eq!(first.samples[0], 3);
    assert_eq!(first.duration(), Duration::from_secs(1));
}

#[test]
fn disabling_discards_the_queue() {
    let tap = AudioTap::new();
    tap.set_enabled(true);
    tap.push(8_000, 1, &[1]);
    tap.set_enabled(false);
    tap.set_enabled(true);
    assert!(tap.pop().is_none());
}

#[test]
fn muting_is_shared_and_off_at_first() {
    let tap = AudioTap::new();
    let backend_side = tap.clone();
    assert!(!backend_side.is_muted());
    tap.set_muted(true);
    assert!(backend_side.is_muted());
    tap.set_muted(false);
    assert!(!backend_side.is_muted());
}

#[test]
fn only_audible_samples_count_as_sound() {
    let tap = AudioTap::new();
    assert!(tap.sound_age().is_none());
    tap.note_played(&[0, AUDIBLE, -AUDIBLE]);
    assert!(tap.sound_age().is_none(), "the noise floor isn't sound");
    tap.note_played(&[0, AUDIBLE + 1]);
    assert!(tap.sound_age().expect("heard") < Duration::from_secs(1));
}

#[test]
fn sound_is_noted_while_disabled_and_muted() {
    let tap = AudioTap::new();
    tap.set_muted(true);
    tap.note_played(&[i16::MIN]);
    assert!(tap.sound_age().is_some());
    assert!(tap.pop().is_none());
}
