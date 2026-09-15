/// Integration bench: measures SQLite write throughput under different configs.
/// Uses synthetic data to avoid dependency on a pre-built kernel DB.
///
/// Run:
///   cargo test -p ctxtag-core --test bench_write -- --nocapture bench_write_throughput

use rusqlite::params;
use rusqlite::Connection;
use std::time::Instant;

#[test]
fn bench_write_throughput() {
    let tmp_db = "/tmp/ctxtag_bench.db";

    // --- Generate synthetic data ---
    println!("\n=== Generating synthetic data ===");
    let gen_start = Instant::now();

    let symbols: Vec<_> = (0..500_000u64).map(|i| {
        (
            format!("symbol_{}", i),
            format!("::ns::symbol_{}", i),
            if i % 3 == 0 { "function" } else if i % 3 == 1 { "variable" } else { "class" },
            i % 2,
            (i % 10000) as i64,
            (i % 5000) as i32,
            ((i % 5000) + 10) as i32,
            (i % 80) as i32,
            ((i % 80) + 10) as i32,
            format!("void symbol_{}(int a, int b) {{ /* body {} */ }}", i, i),
        )
    }).collect();
    let n_sym = symbols.len();
    println!("  symbols: {} rows", n_sym);

    let refs: Vec<_> = (0..1_000_000u64).map(|i| {
        (
            (i % 500_000) as i64,
            format!("symbol_{}", i % 500_000),
            (i % 10000) as i64,
            (i % 3000) as i32,
            (i % 60) as i32,
            if i % 4 == 0 { "call" } else if i % 4 == 1 { "access" } else if i % 4 == 2 { "declaration" } else { "assignment" },
        )
    }).collect();
    let n_ref = refs.len();
    println!("  refs:    {} rows", n_ref);

    let macros_data: Vec<_> = (0..200_000u64).map(|i| {
        (
            format!("MACRO_{}", i),
            if i % 2 == 0 { 1 } else { 0 },
            format!("do {{ /* body {} */ }} while(0)", i),
            (i % 10000) as i64,
            (i % 2000) as i32,
        )
    }).collect();
    let n_macro = macros_data.len();
    println!("  macros:  {} rows", n_macro);
    println!("  Gen time: {:.1}s\n", gen_start.elapsed().as_secs_f32());

    // --- CONFIG A: Current (WAL + NORMAL) ---
    println!("=== A: WAL + synchronous=NORMAL + indexes during insert ===");
    let _ = std::fs::remove_file(tmp_db);
    let mut conn = make_conn(tmp_db, Config::Current);
    create_schema(&conn, IndexTiming::During);

    let (sym_a, _) = bench_insert_symbols(&mut conn, &symbols);
    let (ref_a, _) = bench_insert_refs(&mut conn, &refs);
    let (macro_a, _) = bench_insert_macros_fn(&mut conn, &macros_data);
    println!("  Symbols:  {:.1}s ({:.0} r/s)", sym_a.as_secs_f32(), n_sym as f64 / sym_a.as_secs_f64());
    println!("  Refs:     {:.1}s ({:.0} r/s)", ref_a.as_secs_f32(), n_ref as f64 / ref_a.as_secs_f64());
    println!("  Macros:   {:.1}s ({:.0} r/s)", macro_a.as_secs_f32(), n_macro as f64 / macro_a.as_secs_f64());
    drop(conn);

    // --- CONFIG B: Aggressive pragmas ---
    println!("\n=== B: WAL + sync=OFF + cache=500MB + pg=4096 + indexes during ===");
    let _ = std::fs::remove_file(tmp_db);
    let mut conn = make_conn(tmp_db, Config::Aggressive);
    create_schema(&conn, IndexTiming::During);

    let (sym_b, _) = bench_insert_symbols(&mut conn, &symbols);
    let (ref_b, _) = bench_insert_refs(&mut conn, &refs);
    let (macro_b, _) = bench_insert_macros_fn(&mut conn, &macros_data);
    println!("  Symbols:  {:.1}s ({:.0} r/s)", sym_b.as_secs_f32(), n_sym as f64 / sym_b.as_secs_f64());
    println!("  Refs:     {:.1}s ({:.0} r/s)", ref_b.as_secs_f32(), n_ref as f64 / ref_b.as_secs_f64());
    println!("  Macros:   {:.1}s ({:.0} r/s)", macro_b.as_secs_f32(), n_macro as f64 / macro_b.as_secs_f64());
    drop(conn);

    // --- CONFIG C: Aggressive + indexes AFTER bulk insert ---
    println!("\n=== C: Aggressive + indexes AFTER bulk insert ===");
    let _ = std::fs::remove_file(tmp_db);
    let mut conn = make_conn(tmp_db, Config::Aggressive);
    create_schema(&conn, IndexTiming::After);

    let (sym_c, _) = bench_insert_symbols(&mut conn, &symbols);
    let (ref_c, _) = bench_insert_refs(&mut conn, &refs);
    let (macro_c, _) = bench_insert_macros_fn(&mut conn, &macros_data);
    println!("  Symbols:  {:.1}s ({:.0} r/s)", sym_c.as_secs_f32(), n_sym as f64 / sym_c.as_secs_f64());
    println!("  Refs:     {:.1}s ({:.0} r/s)", ref_c.as_secs_f32(), n_ref as f64 / ref_c.as_secs_f64());
    println!("  Macros:   {:.1}s ({:.0} r/s)", macro_c.as_secs_f32(), n_macro as f64 / macro_c.as_secs_f64());

    let idx_start = Instant::now();
    create_indexes(&conn);
    let idx_elapsed_c = idx_start.elapsed();
    println!("  Index build: {:.1}s", idx_elapsed_c.as_secs_f32());
    drop(conn);

    // --- CONFIG D: Aggressive + no indexes (pure insert baseline) ---
    println!("\n=== D: Aggressive + no indexes (pure insert baseline) ===");
    let _ = std::fs::remove_file(tmp_db);
    let mut conn = make_conn(tmp_db, Config::Aggressive);
    create_schema_no_index(&conn);

    let (sym_d, _) = bench_insert_symbols(&mut conn, &symbols);
    let (ref_d, _) = bench_insert_refs(&mut conn, &refs);
    let (macro_d, _) = bench_insert_macros_fn(&mut conn, &macros_data);
    println!("  Symbols:  {:.1}s ({:.0} r/s)", sym_d.as_secs_f32(), n_sym as f64 / sym_d.as_secs_f64());
    println!("  Refs:     {:.1}s ({:.0} r/s)", ref_d.as_secs_f32(), n_ref as f64 / ref_d.as_secs_f64());
    println!("  Macros:   {:.1}s ({:.0} r/s)", macro_d.as_secs_f32(), n_macro as f64 / macro_d.as_secs_f64());
    drop(conn);

    // --- Cleanup ---
    let _ = std::fs::remove_file(tmp_db);
    let _ = std::fs::remove_file(&format!("{}.wal", tmp_db));
    let _ = std::fs::remove_file(&format!("{}.shm", tmp_db));

    // --- Summary ---
    let total_a = sym_a.as_secs_f64() + ref_a.as_secs_f64() + macro_a.as_secs_f64();
    let total_b = sym_b.as_secs_f64() + ref_b.as_secs_f64() + macro_b.as_secs_f64();
    let total_c = sym_c.as_secs_f64() + ref_c.as_secs_f64() + macro_c.as_secs_f64() + idx_elapsed_c.as_secs_f64();
    let total_d = sym_d.as_secs_f64() + ref_d.as_secs_f64() + macro_d.as_secs_f64();

    println!("\n=== SUMMARY ===");
    println!("{:<24} {:>8} {:>8} {:>8} {:>8}", "", "Sym", "Ref", "Mac", "Total");
    println!("{:<24} {:>7}s {:>7}s {:>7}s {:>7}s", "A current", sym_a.as_secs_f32(), ref_a.as_secs_f32(), macro_a.as_secs_f32(), total_a);
    println!("{:<24} {:>7}s {:>7}s {:>7}s {:>7}s", "B sync=OFF+big_cache", sym_b.as_secs_f32(), ref_b.as_secs_f32(), macro_b.as_secs_f32(), total_b);
    println!("{:<24} {:>7}s {:>7}s {:>7}s {:>7}s", "C idx-after-insert", sym_c.as_secs_f32(), ref_c.as_secs_f32(), macro_c.as_secs_f32(), total_c);
    println!("{:<24} {:>7}s {:>7}s {:>7}s {:>7}s", "D no-indexes", sym_d.as_secs_f32(), ref_d.as_secs_f32(), macro_d.as_secs_f32(), total_d);

    println!("\n=== SPEEDUP vs A ===");
    println!("B: {:.2}x total  (ref: {:.2}x, sym: {:.2}x)", total_a / total_b, ref_a.as_secs_f64() / ref_b.as_secs_f64(), sym_a.as_secs_f64() / sym_b.as_secs_f64());
    println!("C: {:.2}x total  (insert+idx vs insert+idx); index build alone: {:.1}s", total_a / total_c, idx_elapsed_c.as_secs_f32());
    println!("D: {:.2}x total  (pure insert, no index cost); sym: {:.2}x, ref: {:.2}x", total_a / total_d, sym_a.as_secs_f64() / sym_d.as_secs_f64(), ref_a.as_secs_f64() / ref_d.as_secs_f64());

    // Estimate: full kernel with config C
    let kernel_total_est = 2296.4;
    println!("\n=== PROJECTED KERNEL TIME (2296s baseline) ===");
    println!("With B: {:.0}s ({:.1}x)", kernel_total_est * (total_b / total_a), total_a / total_b);
    println!("With C: {:.0}s ({:.1}x)", kernel_total_est * (total_c / total_a), total_a / total_c);
    println!("With D: {:.0}s ({:.1}x) -- needs rebuild-on-read index", kernel_total_est * (total_d / total_a), total_a / total_d);
}

// ─── Helpers ────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Config { Current, Aggressive }
enum IndexTiming { During, After }

fn make_conn(path: &str, config: Config) -> Connection {
    let conn = Connection::open(path).expect("open");
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.pragma_update(None, "foreign_keys", "OFF").ok();
    match config {
        Config::Current => {
            conn.pragma_update(None, "synchronous", "NORMAL").ok();
        }
        Config::Aggressive => {
            conn.pragma_update(None, "synchronous", "OFF").ok();
            conn.pragma_update(None, "cache_size", "500000").ok();
            conn.pragma_update(None, "page_size", "4096").ok();
            conn.pragma_update(None, "temp_store", "memory").ok();
            conn.pragma_update(None, "mmap_size", "2147483648").ok();
        }
    }
    conn
}

fn create_schema(conn: &Connection, idx: IndexTiming) {
    conn.execute_batch(
        "CREATE TABLE symbols (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, qualified_name TEXT NOT NULL, kind TEXT NOT NULL, is_definition INTEGER, file_id INTEGER NOT NULL, line_start INTEGER NOT NULL, line_end INTEGER NOT NULL, column_start INTEGER NOT NULL, column_end INTEGER NOT NULL, declaration_text TEXT);
         CREATE TABLE `references` (id INTEGER PRIMARY KEY AUTOINCREMENT, symbol_id INTEGER NOT NULL, symbol_name TEXT NOT NULL, file_id INTEGER NOT NULL, line INTEGER NOT NULL, column INTEGER NOT NULL, ref_kind TEXT NOT NULL);
         CREATE TABLE macros (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, is_function_like INTEGER, body TEXT, file_id INTEGER NOT NULL, line INTEGER NOT NULL);",
    ).ok();
    if matches!(idx, IndexTiming::During) {
        create_indexes(conn);
    }
}

fn create_schema_no_index(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE symbols (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, qualified_name TEXT NOT NULL, kind TEXT NOT NULL, is_definition INTEGER, file_id INTEGER NOT NULL, line_start INTEGER NOT NULL, line_end INTEGER NOT NULL, column_start INTEGER NOT NULL, column_end INTEGER NOT NULL, declaration_text TEXT);
         CREATE TABLE `references` (id INTEGER PRIMARY KEY AUTOINCREMENT, symbol_id INTEGER NOT NULL, symbol_name TEXT NOT NULL, file_id INTEGER NOT NULL, line INTEGER NOT NULL, column INTEGER NOT NULL, ref_kind TEXT NOT NULL);
         CREATE TABLE macros (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, is_function_like INTEGER, body TEXT, file_id INTEGER NOT NULL, line INTEGER NOT NULL);",
    ).ok();
}

fn create_indexes(conn: &Connection) {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_sym_name ON symbols(name);
         CREATE INDEX IF NOT EXISTS idx_sym_qn ON symbols(qualified_name);
         CREATE INDEX IF NOT EXISTS idx_sym_file ON symbols(file_id);
         CREATE INDEX IF NOT EXISTS idx_ref_sym ON `references`(symbol_id);
         CREATE INDEX IF NOT EXISTS idx_ref_sym_file ON `references`(symbol_id, file_id);
         CREATE INDEX IF NOT EXISTS idx_ref_file_pos ON `references`(file_id, line, column);
         CREATE INDEX IF NOT EXISTS idx_macros_name ON macros(name);",
    ).ok();
}

fn bench_insert_symbols(conn: &mut Connection, data: &[(String, String, &str, u64, i64, i32, i32, i32, i32, String)]) -> (std::time::Duration, usize) {
    let start = Instant::now();
    let tx = conn.transaction().expect("tx");
    let mut count = 0;
    for (name, qn, kind, is_def, file_id, ls, le, cs, ce, decl) in data {
        tx.execute(
            "INSERT INTO symbols (name, qualified_name, kind, is_definition, file_id, line_start, line_end, column_start, column_end, declaration_text) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![name, qn, kind, *is_def as i32, file_id, ls, le, cs, ce, decl],
        ).expect("insert");
        count += 1;
    }
    drop(tx);
    (start.elapsed(), count)
}

fn bench_insert_refs(conn: &mut Connection, data: &[(i64, String, i64, i32, i32, &str)]) -> (std::time::Duration, usize) {
    let start = Instant::now();
    let tx = conn.transaction().expect("tx");
    let mut count = 0;
    for (sym_id, sym_name, file_id, line, col, kind) in data {
        tx.execute(
            "INSERT INTO `references` (symbol_id, symbol_name, file_id, line, column, ref_kind) VALUES (?, ?, ?, ?, ?, ?)",
            params![sym_id, sym_name, file_id, line, col, kind],
        ).expect("insert");
        count += 1;
    }
    drop(tx);
    (start.elapsed(), count)
}

fn bench_insert_macros_fn(conn: &mut Connection, data: &[(String, i32, String, i64, i32)]) -> (std::time::Duration, usize) {
    let start = Instant::now();
    let tx = conn.transaction().expect("tx");
    let mut count = 0;
    for (name, is_fn, body, file_id, line) in data {
        tx.execute(
            "INSERT INTO macros (name, is_function_like, body, file_id, line) VALUES (?, ?, ?, ?, ?)",
            params![name, is_fn, body, file_id, line],
        ).expect("insert");
        count += 1;
    }
    drop(tx);
    (start.elapsed(), count)
}
