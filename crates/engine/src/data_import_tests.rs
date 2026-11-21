use super::*;
use anicraft_doc::data::{MAX_CELLS, MAX_COLUMNS, MAX_ROWS};

fn names(p: &Parsed) -> Vec<&str> {
    p.columns.iter().map(|c| c.name.as_str()).collect()
}

fn col<'a>(p: &'a Parsed, name: &str) -> &'a [Option<f64>] {
    &p.columns.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}: {:?}", names(p))).values
}

#[test]
fn numbers_are_read_plainly() {
    for (s, v) in [("1", 1.0), (" -2.5 ", -2.5), ("+3", 3.0), (".5", 0.5), ("1e3", 1000.0), ("2.5E-1", 0.25), ("45%", 45.0), ("12 %", 12.0)] {
        assert_eq!(parse_number(s, false), Some(v), "{s:?}");
    }
    for s in ["", " ", "abc", "nan", "NaN", "inf", "-Infinity", "1e999", "1,5", "1 2", "0x10", "--1", "e", "%", "١٢"] {
        assert_eq!(parse_number(s, false), None, "{s:?}");
    }
    // A decimal comma, where the comma can't be the delimiter.
    assert_eq!(parse_number("1,5", true), Some(1.5));
    assert_eq!(parse_number("1.234,5", true), None);
    assert_eq!(parse_number("1,2,3", true), None);
    assert_eq!(parse_number(&"9".repeat(100), false), None, "an endless digit string is not a number");
}

#[test]
fn csv_with_a_header_gaps_and_a_text_column() {
    let p = parse(b"sample,depth,pinus,note\r\nA1,0.5,12,first\r\nA2,1.5,,\r\nA3,2.5,n/a,third\r\n", "core.csv", None).unwrap();
    assert_eq!(names(&p), ["depth", "pinus"]);
    assert_eq!(p.skipped, ["sample", "note"], "columns with no number are left out and reported");
    assert_eq!(col(&p, "depth"), [Some(0.5), Some(1.5), Some(2.5)]);
    assert_eq!(col(&p, "pinus"), [Some(12.0), None, None], "an empty cell and text are gaps");
    assert_eq!((p.rows(), p.gaps, p.truncated), (3, 2, false));
}

#[test]
fn csv_edge_cases() {
    // Quoted fields, a quoted delimiter and line break, a byte order mark, blank lines, no
    // newline at the end.
    let p = parse("\u{feff}\"a, b\",\"c\nd\"\n\n\"1\",\"2\"\n   \n3,4".as_bytes(), "x.csv", None).unwrap();
    assert_eq!(names(&p), ["a, b", "cd"], "a line break in a name is dropped");
    assert_eq!(p.columns[0].values, [Some(1.0), Some(3.0)]);
    assert_eq!(p.columns[1].values, [Some(2.0), Some(4.0)]);
    // No header: every field of the first record is a number.
    let p = parse(b"1,2\n3,4\n", "x.csv", None).unwrap();
    assert_eq!(names(&p), ["Column 1", "Column 2"]);
    assert_eq!(p.rows(), 2);
    // Ragged rows: short ones are padded with gaps, long ones grow the table.
    let p = parse(b"a,b\n1\n2,3,4\n", "x.csv", None).unwrap();
    assert_eq!(names(&p), ["a", "b", "Column 3"]);
    assert_eq!(col(&p, "b"), [None, Some(3.0)]);
    assert_eq!(col(&p, "Column 3"), [None, Some(4.0)]);
    // Repeated and empty names each keep their own column (the table makes them unique).
    let p = parse(b"v,v,\n1,2,3\n", "x.csv", None).unwrap();
    assert_eq!(p.columns.len(), 3);
    // A semicolon file with decimal commas; a tab file by its extension.
    let p = parse(b"age;value\n1,5;2,25\n", "x.csv", None).unwrap();
    assert_eq!(col(&p, "age"), [Some(1.5)]);
    assert_eq!(col(&p, "value"), [Some(2.25)]);
    let p = parse(b"age\tvalue\n1\t2\n", "x.tsv", None).unwrap();
    assert_eq!(col(&p, "value"), [Some(2.0)]);
    let p = parse(b"a b\tc,d\n1\t2\n", "x.txt", Some(Format::Tsv)).unwrap();
    assert_eq!(names(&p), ["a b", "c,d"]);
    // Windows-1252 text in a header still reads.
    let p = parse(b"temp\xe9rature\n3\n", "x.csv", None).unwrap();
    assert_eq!(names(&p), ["temp\u{e9}rature"]);
}

#[test]
fn files_without_numbers_are_refused() {
    for bytes in [&b""[..], b"\n\n\n", b"a,b\n", b"a,b\nx,y\n", b"\x00\x01\x02\xff\xfe", b"[]", b"{}", b"{\"a\": []}", b"[[\"a\", \"b\"]]"] {
        assert!(parse(bytes, "x.csv", None).is_err(), "{bytes:?}");
    }
    for bytes in [&b"{"[..], b"[1, 2", b"nope", b"3", b"\"text\"", b"[1] trailing"] {
        assert!(parse(bytes, "x.json", None).is_err(), "{bytes:?}");
    }
}

#[test]
fn json_in_every_shape() {
    // Records.
    let p = parse(br#"[{"age": 1, "pinus": 10, "id": "a"}, {"age": 2, "pinus": null}, {"pinus": "30", "age": 3, "extra": 7}]"#, "x.json", None).unwrap();
    assert_eq!(names(&p), ["age", "pinus", "extra"]);
    assert_eq!(col(&p, "pinus"), [Some(10.0), None, Some(30.0)], "a numeric string is a number");
    assert_eq!(col(&p, "extra"), [None, None, Some(7.0)]);
    assert_eq!(p.skipped, ["id"]);
    // Rows with a header row; nested values are gaps.
    let p = parse(br#"[["age", "v"], [1, 2], [3, [4]], [5, {"x": 1}], [6]]"#, "x.json", None).unwrap();
    assert_eq!(names(&p), ["age", "v"]);
    assert_eq!(col(&p, "v"), [Some(2.0), None, None, None]);
    // Rows without a header.
    let p = parse(b"[[1, 2], [3, 4]]", "x.json", None).unwrap();
    assert_eq!(names(&p), ["Column 1", "Column 2"]);
    // Columns, of different lengths.
    let p = parse(br#"{"age": [1, 2, 3], "v": [10, true], "label": ["a", "b"], "meta": {"x": 1}, "n": 4}"#, "x.json", None).unwrap();
    assert_eq!(names(&p), ["age", "v"]);
    assert_eq!(col(&p, "v"), [Some(10.0), None, None]);
    // Names and rows.
    let p = parse(br#"{"rows": [[1, 2], [3, 4]], "columns": ["a", "b"]}"#, "x.json", None).unwrap();
    assert_eq!(names(&p), ["a", "b"]);
    assert_eq!(col(&p, "b"), [Some(2.0), Some(4.0)]);
    // A plain list of numbers is one column.
    let p = parse(b"[3, 1, null, 2]", "x.json", None).unwrap();
    assert_eq!(names(&p), ["value"]);
    assert_eq!(col(&p, "value"), [Some(3.0), Some(1.0), None, Some(2.0)]);
    // Detected by its first character when the extension says nothing.
    assert_eq!(parse(b"  [[1, 2]]", "data.txt", None).unwrap().rows(), 1);
    // A number too large for a table is refused by the reader, and never an infinity.
    assert!(parse(b"[1e400, 1, -1e400]", "x.json", None).is_err());
    let p = parse(b"[\"1e400\", 1, 1e308]", "x.json", None).unwrap();
    assert_eq!(col(&p, "value"), [None, Some(1.0), Some(1e308)]);
}

#[test]
fn hostile_tables_are_cut_to_the_limits() {
    // More rows than a table keeps.
    let mut csv = String::from("v\n");
    for i in 0..MAX_ROWS + 50 {
        csv.push_str(&format!("{i}\n"));
    }
    let p = parse(csv.as_bytes(), "x.csv", None).unwrap();
    assert_eq!((p.rows(), p.truncated), (MAX_ROWS, true));
    // More columns.
    let wide = format!("{}\n", (0..MAX_COLUMNS + 40).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    let p = parse(wide.repeat(3).as_bytes(), "x.csv", None).unwrap();
    assert_eq!((p.columns.len(), p.truncated), (MAX_COLUMNS, true));
    // More cells: 200 columns keep MAX_CELLS / 200 rows.
    let row = format!("{}\n", vec!["1"; 200].join(","));
    let p = parse(row.repeat(MAX_CELLS / 200 + 10).as_bytes(), "x.csv", None).unwrap();
    assert_eq!((p.rows(), p.truncated), (MAX_CELLS / 200, true));
    // The same through JSON: rows, a wide row, and columns.
    let json = format!("[{}]", vec!["[1,2]"; MAX_ROWS + 10].join(","));
    let p = parse(json.as_bytes(), "x.json", None).unwrap();
    assert_eq!((p.rows(), p.truncated), (MAX_ROWS, true));
    let json = format!("[[{}]]", vec!["1"; MAX_COLUMNS * 3].join(","));
    let p = parse(json.as_bytes(), "x.json", None).unwrap();
    assert_eq!((p.columns.len(), p.truncated), (MAX_COLUMNS, true));
    let json = format!("{{{}}}", (0..40).map(|i| format!("\"c{i}\": [{}]", vec!["1"; 60_000].join(","))).collect::<Vec<_>>().join(","));
    let p = parse(json.as_bytes(), "x.json", None).unwrap();
    assert!(p.truncated && p.rows() * p.columns.len() <= MAX_CELLS, "{} x {}", p.rows(), p.columns.len());
    // Deep nesting is refused by the reader, not the stack.
    let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(parse(deep.as_bytes(), "x.json", None).is_err());
    // An endless quoted field, a row of delimiters, and control characters are just text.
    assert!(parse(format!("a\n\"{}", "x".repeat(200_000)).as_bytes(), "x.csv", None).is_err());
    assert!(parse(",".repeat(500_000).as_bytes(), "x.csv", None).is_err());
    let p = parse(b"a\x00\x07b,c\n1,2\n", "x.csv", None).unwrap();
    assert_eq!(p.columns.len(), 2);
}

#[test]
fn the_reader_never_panics_on_junk() {
    // A small deterministic fuzz over bytes that look like tables.
    let alphabet: &[u8] = b"0123456789.,;\t\n\r\"-+eE%[]{}: anx\\\xff\x00";
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..3_000 {
        let len = (next() % 200) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        for name in ["x.csv", "x.tsv", "x.json", "x"] {
            if let Ok(p) = parse(&bytes, name, None) {
                let rows = p.rows();
                assert!(!p.columns.is_empty() && p.columns.iter().all(|c| c.values.len() == rows && c.values.iter().all(|v| v.is_none_or(f64::is_finite))), "round {round} {name}");
            }
        }
    }
}
