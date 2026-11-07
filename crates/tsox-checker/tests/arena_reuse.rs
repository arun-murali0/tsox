use tsox_base::file_id::FileId;
use tsox_checker::bridge::check_program_with_state;
use tsox_types::arena::TypeArena;

fn check(source: &str, file_name: &str, arena: &mut TypeArena) -> String {
    let (diagnostics, _) = check_program_with_state(source, file_name, FileId::ROOT, arena)
        .expect("fixture should parse and check");
    format!("{diagnostics:#?}")
}

#[test]
fn reused_arena_matches_a_fresh_arena_after_enum_and_generic_metadata() {
    let producer = include_str!("fixtures/arena-reuse/metadata_producer.ts");
    let consumer = include_str!("fixtures/arena-reuse/metadata_consumer.ts");

    let mut reused = TypeArena::new();
    let producer_diagnostics = check(producer, "metadata_producer.ts", &mut reused);
    assert_eq!(
        producer_diagnostics, "[]",
        "producer fixture: {producer_diagnostics}"
    );

    let reused_diagnostics = check(consumer, "metadata_consumer.ts", &mut reused);
    let mut fresh = TypeArena::new();
    let fresh_diagnostics = check(consumer, "metadata_consumer.ts", &mut fresh);

    assert_eq!(
        reused_diagnostics, fresh_diagnostics,
        "checking the consumer after the producer must match a fresh arena"
    );
    assert_eq!(
        reused_diagnostics, "[]",
        "consumer fixture: {reused_diagnostics}"
    );
}
