# Machineplane Test Specification

This document defines the test scenarios for machineplane scheduler protocol.

---

## Unit Tests

### Manifest Parsing (manifest_tests.rs)

**manifest_name_extraction_returns_name_when_present**
- Given a YAML manifest with metadata.name set
- When extracting the manifest name
- Then the name is returned successfully

**manifest_name_extraction_handles_missing_fields**
- Given a YAML manifest without metadata or metadata.name
- When extracting the manifest name
- Then an appropriate error is returned

**manifest_id_hashing_is_deterministic**
- Given the same manifest content
- When computing the manifest ID multiple times
- Then the same ID is returned each time

### CLI Environment (cli_tests.rs)

**container_host_env_sets_podman_socket**
- Given CONTAINER_HOST environment variable is set
- When initializing the Podman client
- Then the socket path from CONTAINER_HOST is used

### Scheduler Protocol (scheduler_tests.rs)

#### Bid Scoring

**bid_score_computation_is_deterministic**
- Given the same bid parameters
- When computing the score multiple times
- Then the same score is returned

**resource_fit_score_is_bounded**
- Given any valid bid parameters
- When computing the resource fit score
- Then the score is within [0.0, 1.0]

#### Winner Selection

**winner_selection_is_deterministic**
- Given the same set of bids
- When selecting winners multiple times
- Then the same winners are chosen

**winner_selection_uses_peer_id_as_tiebreaker**
- Given bids with identical scores
- When selecting a winner
- Then the peer ID is used as tiebreaker

**winner_selection_deduplicates_by_node**
- Given multiple bids from the same node
- When selecting winners
- Then only one bid per node is considered

#### Timestamp Freshness

**timestamp_freshness_accepts_current_time**
- Given a message with current timestamp
- When checking freshness
- Then the message is accepted

**timestamp_freshness_accepts_within_skew_window**
- Given a message within the skew window
- When checking freshness
- Then the message is accepted

**timestamp_freshness_rejects_stale**
- Given a message with old timestamp
- When checking freshness
- Then the message is rejected

**timestamp_freshness_rejects_replay_attacks**
- Given a message with future timestamp
- When checking freshness
- Then the message is rejected

#### Replay Protection

**replay_filter_accepts_first_occurrence**
- Given a new nonce
- When checking the replay filter
- Then the message is accepted

**replay_filter_rejects_duplicates**
- Given a previously seen nonce
- When checking the replay filter
- Then the message is rejected

**replay_filter_accepts_different_nonces**
- Given different nonces from the same tender
- When checking the replay filter
- Then both messages are accepted

**replay_filter_accepts_different_tender_ids**
- Given the same nonce but different tender IDs
- When checking the replay filter
- Then both messages are accepted

#### Signature Verification - Tender

**signature_verification_accepts_valid_tender**
- Given a properly signed tender
- When verifying the signature
- Then verification succeeds

**signature_verification_rejects_tampered_tender**
- Given a tender with modified content
- When verifying the signature
- Then verification fails

**signature_verification_rejects_unsigned_tender**
- Given a tender without signature
- When verifying the signature
- Then verification fails

**signature_verification_rejects_wrong_key_tender**
- Given a tender signed by a different key
- When verifying with the expected key
- Then verification fails

#### Signature Verification - Bid

**signature_verification_accepts_valid_bid**
- Given a properly signed bid
- When verifying the signature
- Then verification succeeds

**signature_verification_rejects_tampered_bid**
- Given a bid with modified content
- When verifying the signature
- Then verification fails

#### Signature Verification - Award

**signature_verification_accepts_valid_award**
- Given a properly signed award
- When verifying the signature
- Then verification succeeds

**signature_verification_rejects_tampered_award**
- Given an award with modified content
- When verifying the signature
- Then verification fails

#### Signature Verification - Event

**signature_verification_accepts_valid_event**
- Given a properly signed event
- When verifying the signature
- Then verification succeeds

**signature_verification_rejects_tampered_event**
- Given an event with modified content
- When verifying the signature
- Then verification fails

#### Protocol Constants

**selection_window_is_reasonable**
- Given the selection window constant
- When checking its value
- Then it is between 1-10 seconds

**all_event_types_can_be_signed**
- Given all event type variants
- When signing each type
- Then all sign successfully

**fabric_topic_constant_is_correct**
- Given the FABRIC_TOPIC constant
- When checking its value
- Then it matches the expected topic name

**topic_hash_comparison_filters_wrong_topic**
- Given a message on a different topic
- When comparing topic hashes
- Then the message is filtered out

**topic_filtering_is_case_sensitive**
- Given topic names with different cases
- When comparing topic hashes
- Then they are treated as different topics

#### Message Structure

**tender_signature_covers_all_fields**
- Given a signed tender
- When modifying any field
- Then signature verification fails

**bid_signature_covers_all_fields**
- Given a signed bid
- When modifying any field
- Then signature verification fails

**award_signature_covers_all_fields**
- Given a signed award
- When modifying any field
- Then signature verification fails

**tender_contains_digest_not_manifest**
- Given a tender message
- When inspecting its structure
- Then it contains manifest digest, not full manifest

**bid_nonces_are_unique**
- Given multiple bids created
- When comparing nonces
- Then each bid has a unique nonce

**bid_contains_node_id**
- Given a bid message
- When inspecting its structure
- Then it contains the bidding node's ID

**award_has_winners**
- Given an auction with bids
- When creating an award
- Then it contains winner information

**award_can_have_no_winners**
- Given an auction with no qualifying bids
- When creating an award
- Then it contains empty winner list

---

## Integration Tests

> Note: Integration tests run in CI with Podman. Some tests require
> specific Podman versions for full compatibility.

### Apply Command (apply_tests.rs)

**apply_deploys_manifest_to_mesh**
- Given a valid Kubernetes manifest
- When running apply command
- Then the workload is scheduled to the mesh

**apply_with_podman_creates_and_removes_pods**
- Given a manifest and running Podman daemon
- When applying and then deleting the manifest
- Then pods are created and then cleaned up

**apply_distributes_replicas_across_nodes**
- Given a manifest with replicas > 1
- When applying to a multi-node mesh
- Then replicas are distributed to different nodes

### Mesh Formation (mesh_tests.rs)

**mesh_forms_and_endpoints_respond**
- Given multiple machineplane nodes
- When they join the same mesh
- Then they discover each other and form a cluster
