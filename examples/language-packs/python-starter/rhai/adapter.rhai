// A script-only diagnostic fact in addition to the query templates.
// `context` and `matches` are immutable host inputs.
let records = [];
for m in matches {
    if m.query == "functions" {
        for c in m.captures {
            if c.name == "name" {
                records.push(#{"kind":"fact", "name":"script_seen", "args":[c.text], "span":c.span});
            }
        }
    }
}
records
