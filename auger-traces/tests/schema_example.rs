use auger_traces::schema::TraceRecord;

#[test]
fn example_deserializes() {
    for line in include_str!("../trace_format.jsonl").lines() {
        serde_json::from_str::<TraceRecord>(line).unwrap();
    }
}
