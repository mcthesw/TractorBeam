use super::*;

#[test]
fn preferred_healthy_candidate_pair_wins() {
    let preferred_local = "127.0.0.1:21001".parse().unwrap();
    let fallback_local = "127.0.0.1:21002".parse().unwrap();
    let preferred_remote = "127.0.0.1:22001".parse().unwrap();
    let fallback_remote = "127.0.0.1:22002".parse().unwrap();
    let successful = CheckState {
        request_seen: true,
        response_seen: true,
    };
    let checks = BTreeMap::from([
        ((preferred_local, preferred_remote), successful),
        ((fallback_local, fallback_remote), successful),
    ]);
    let local = HashMap::from([(preferred_local, 20), (fallback_local, 10)]);
    let remote = HashMap::from([(preferred_remote, 20), (fallback_remote, 10)]);

    assert_eq!(
        select_candidate_pair(&checks, &local, &remote),
        Some((preferred_local, preferred_remote))
    );
}

#[test]
fn only_bidirectionally_successful_pair_is_eligible() {
    let preferred_local = "127.0.0.1:21001".parse().unwrap();
    let fallback_local = "127.0.0.1:21002".parse().unwrap();
    let preferred_remote = "127.0.0.1:22001".parse().unwrap();
    let fallback_remote = "127.0.0.1:22002".parse().unwrap();
    let checks = BTreeMap::from([
        (
            (preferred_local, preferred_remote),
            CheckState {
                request_seen: false,
                response_seen: true,
            },
        ),
        (
            (fallback_local, fallback_remote),
            CheckState {
                request_seen: true,
                response_seen: true,
            },
        ),
    ]);
    let local = HashMap::from([(preferred_local, 20), (fallback_local, 10)]);
    let remote = HashMap::from([(preferred_remote, 20), (fallback_remote, 10)]);

    assert_eq!(
        select_candidate_pair(&checks, &local, &remote),
        Some((fallback_local, fallback_remote))
    );
}
