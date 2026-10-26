use super::*;

#[test]
fn closed_sources_and_cancelled_jobs_release_only_their_owned_admission() -> Result<()> {
    fn send<T: Send>() {}
    send::<Session>();
    let mut host = Host::new(config())?;
    assert_eq!(host.call(41, &creation(8192))?[0], 169);
    assert_eq!(host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?[0], 173);
    let wait = host.event(1)?;
    assert_eq!(wait[0], 179);
    assert_eq!(wide(&wait, 45)?, 0);
    host.call(44, &words(&[1]))?;
    let retained = host.call(52, &[])?;
    assert_eq!(word(&retained, 5)?, 0);
    assert_eq!(
        wide(&retained, 13)?,
        pagura::convert::usize_to_u64(SOURCE_BYTES)
    );
    assert!(wide(&retained, 21)? > 0);
    // Removing a source handle does not refund the opener's frontier/source.
    assert_eq!(word(&host.call(41, &creation(8))?, 5)?, 2);
    assert_eq!(host.call(41, &creation(8))?[0], 255);
    assert_eq!(
        &host.call(50, &words(&[1]))?[5..],
        [words(&[1]), vec![1]].concat()
    );
    assert_eq!(host.event(1)?[9], 7);
    assert_eq!(host.call(50, &words(&[1]))?[9], 0);
    let released = host.call(52, &[])?;
    assert_eq!(
        (wide(&released, 13)?, wide(&released, 21)?),
        (pagura::convert::usize_to_u64(SOURCE_BYTES), 0)
    );
    assert_eq!(word(&host.call(41, &creation(8))?, 5)?, 3);
    host.call(51, &words(&[1]))?;
    assert_eq!(host.call(48, &words(&[1]))?[0], 255);
    Ok(())
}

#[test]
fn finalization_holes_versions_conflicts_and_source_closure_are_terminal() -> Result<()> {
    let mut host = Host::new(config())?;
    host.call(41, &creation(4))?;
    host.supply(0, b"abc")?;
    let mut stale = [
        words(&[1]),
        vec![0; 32],
        0_u64.to_le_bytes().to_vec(),
        words(&[1]),
        b"z".to_vec(),
    ]
    .concat();
    assert_eq!(host.call(42, &stale)?[5], 7);
    assert_eq!(
        &host.supply(0, b"abc")?[5..],
        [0_u64.to_le_bytes(), 1_u64.to_le_bytes()].concat()
    );
    let hole = host.call(43, &words(&[1]))?;
    assert_eq!((hole[0], hole[5]), (255, 7));
    assert!(
        hole.windows(35)
            .any(|bytes| bytes == b"source finalized with missing bytes")
    );
    let still_terminal = host.supply(3, b"d")?;
    assert_eq!((still_terminal[0], still_terminal[5]), (255, 7));
    assert!(
        still_terminal
            .windows(35)
            .any(|bytes| bytes == b"source finalized with missing bytes")
    );
    assert_eq!(&host.call(43, &words(&[1]))?[5..], &hole[5..]);
    host.call(44, &words(&[1]))?;
    assert_eq!(host.call(44, &words(&[1]))?[0], 255);
    host.call(41, &creation(4))?;
    stale[..4].copy_from_slice(&2_u32.to_le_bytes());
    stale[4..36].copy_from_slice(&VERSION);
    assert_eq!(host.call(42, &stale)?[0], 170);
    *stale
        .last_mut()
        .ok_or(Error::syntax("test supply missing byte"))? = b'x';
    assert_eq!(host.call(42, &stale)?[0], 255);
    assert_eq!(host.call(43, &words(&[2]))?[0], 255);
    Ok(())
}

#[test]
fn a_waiting_job_failure_is_stable_and_never_reports_completion() -> Result<()> {
    let mut host = Host::new(config())?;
    host.call(41, &creation(8192))?;
    host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?;
    assert_eq!(host.event(1)?[0], 179);
    host.call(44, &words(&[1]))?;
    let failed = host.event(1)?;
    assert_eq!((failed[0], failed[5]), (255, 7));
    for _ in 0..3 {
        assert_eq!(&host.event(1)?[5..], &failed[5..]);
    }
    assert_eq!(host.call(50, &words(&[1]))?[9], 0);
    let stats = host.call(52, &[])?;
    assert_eq!((wide(&stats, 13)?, wide(&stats, 21)?), (0, 0));
    Ok(())
}

#[test]
fn finalization_does_not_require_whole_file_assembly_and_cancel_discards_output() -> Result<()> {
    let mut small = config();
    let mut limits = small.progressive_limits();
    limits.source.assembly_bytes = 1;
    small = small.with_progressive(limits);
    let mut host = Host::new(small)?;
    host.call(41, &creation(4))?;
    host.supply(2, b"cd")?;
    host.supply(0, b"ab")?;
    assert_eq!(host.call(43, &words(&[1]))?[0], 171);
    assert_eq!(host.call(43, &words(&[1]))?[0], 171);
    assert_eq!(host.supply(0, b"abcd")?[0], 170);

    let mut host = sparse()?;
    host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?;
    host.event(1)?;
    host.supply(1500, b"4 0 obj << /Length 13 >> stream\n")?;
    host.supply(1532, b"0 0 8 8 re f\n")?;
    host.supply(1545, b"\nendstream\nendobj\n")?;
    host.call(47, &rendering(0))?;
    assert_eq!(host.event(2)?[9], 3);
    let band = host.event(2)?;
    assert_eq!(band[9], 4);
    host.call(6, &words(&[1]))?;
    host.call(44, &words(&[1]))?;
    let stats = host.call(52, &[])?;
    assert!(wide(&stats, 21)? > 0);
    assert_eq!(host.call(50, &words(&[2]))?[9], 1);
    assert_eq!(host.event(2)?[9], 7);
    assert_eq!(host.call(49, &ack(2, 1))?[0], 255);
    let stats = host.call(52, &[])?;
    assert_eq!((wide(&stats, 13)?, wide(&stats, 21)?), (0, 0));
    assert_eq!(&band[66..], [0, 0, 0, 255].repeat(64));
    Ok(())
}

#[test]
fn aggregate_admission_and_handle_caps_refuse_before_starting_work() -> Result<()> {
    let mut options = config();
    options = options.with_progressive(ProgressiveLimits {
        active_bytes: 0,
        jobs: 1,
        sources: 1,
        ..options.progressive_limits()
    });
    let mut host = Host::new(options)?;
    assert_eq!(host.call(41, &creation(8))?[0], 169);
    assert_eq!(host.call(41, &creation(8))?[5], 2);
    assert_eq!(host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?[5], 2);
    let stats = host.call(52, &[])?;
    assert_eq!((word(&stats, 9)?, wide(&stats, 21)?), (0, 0));
    let mut host = sparse()?;
    host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?;
    host.event(1)?;
    let mut invalid = rendering(0);
    invalid[8..16].copy_from_slice(&f64::NAN.to_le_bytes());
    assert_eq!(host.call(47, &invalid)?[5], 7);
    assert_eq!(word(&host.call(47, &rendering(0))?, 5)?, 2);
    Ok(())
}

#[test]
fn terminal_job_handles_count_until_closed_and_small_frames_refuse_before_output() -> Result<()> {
    let options = config();
    let mut host = Host::new(options.with_progressive(ProgressiveLimits {
        jobs: 1,
        ..options.progressive_limits()
    }))?;
    host.call(41, &creation(8192))?;
    assert_eq!(host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?[0], 173);
    host.call(50, &words(&[1]))?;
    assert_eq!(host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?[5], 2);
    host.call(51, &words(&[1]))?;
    assert_eq!(
        word(&host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?, 5)?,
        2
    );
    let options = config().with_session(SessionLimits {
        reading_response_bytes: 128,
        ..config().session
    });
    let mut host = Host::new(options)?;
    assert_eq!(host.call(47, &rendering(0))?[5], 2);
    assert_eq!(word(&host.call(52, &[])?, 9)?, 0);
    Ok(())
}
