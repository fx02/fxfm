# fxfm plan 2: arhive kao mape – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enter na `.zip`, `.tar`, `.tgz`/`.tar.gz`, `.gz` i `.7z` ulazi u arhivu kao u mapu. Unutra rade F3, F4 (s "upload back"), F5, F6, F7, F8 i F2, a original arhive se nikad ne kvari.

**Architecture:**
- Nova `fxfm-core` modula:
  - `archive/` čita indeks, raspakirava i prepisuje arhivu (nova privremena arhiva pa atomski `rename`);
  - `vfs.rs` usmjerava svaku lokaciju (`String`) na lokalni disk (postojeći `listing`/`ops`, **nepromijenjeni**) ili na arhivu.
- Operacije koje uključuju arhivu idu preko lokalnog **staging** foldera. Izvor se najprije materijalizira lokalno (raspakira), zatim postojeći `ops::copy` radi lokalni dio s istim konfliktima i progressom, a na kraju jedan `rewrite` upisuje u arhivu. Isti model kasnije koristi plan 3 (server = preuzmi/učitaj).
- **Odstupanje od bilješke "Nakon plana 1"**:
  - `LocalFs` se ne seli iza traita;
  - `ops.rs` (2 175 linija, pregledan i siguran) ostaje netaknut, a routing je u `vfs.rs`;
  - trait se uvodi u planu 3 ako drugi backend to zatraži.
- `src-tauri` komande i `JobRequest` zadržavaju oblik, ali pozivaju `vfs::*`. F4 iz arhive prati privremenu kopiju (`notify`) i pita za upload natrag.

**Tech Stack:**
- sve iz plana 1;
- `zip` 9 (features `deflate`, `deflate64`, `aes-crypto`, `time`; bez default featurea, da nema C ovisnosti);
- `tar` 0.4;
- `flate2` 1 (default miniz_oxide);
- `sevenz-rust2` 0.25 (feature `compress` za pisanje, bez `aes256`);
- `notify` 8 (stabilni, ne 9-rc), samo u `src-tauri`;
- `dirs` (već postoji).

Sve je čisti Rust ili se statički linka, tako da universal build ostaje bez Homebrewa.

**Spec:** `docs/superpowers/specs/2026-11-06-fxfm-design.md` (§2 cilj 6, §4 pravila vraćanja, §5 F3/F4, §8 Arhive). Plan 1: `docs/superpowers/plans/2026-11-07-fxfm-plan1-local.md` (zajednički tipovi i konvencije vrijede i ovdje).

## Global Constraints

- Sve iz Global Constraints plana 1 vrijedi i dalje: Tauri izolacija, `fxfm-core` se builda i testira na Linuxu, commitovi samo kao `zbuzanic@gmail.com` s Co-Authored-By linijom, UI tekstovi na engleskom.
- **Podržani formati** se prepoznaju po imenu, bez obzira na velika i mala slova:
  - `.zip` → `Zip`
  - `.tar` → `Tar`
  - `.tgz` i `.tar.gz` → `TarGz`
  - `.gz` (ali ne `.tar.gz`) → `Gz`
  - `.7z` → `SevenZ`
  - Ostale "arc" ekstenzije iz `ARCHIVE_EXT` (bz2, xz, rar, tbz2, txz) zadržavaju boju, ali se Enterom otvaraju zadanom aplikacijom, kao i dosad.
- **Lokacija u arhivi** je `<putanja do arhive>/<unutarnja putanja>`, npr. `/Users/z/a.zip/src/lib`. Korijen arhive je sama `/Users/z/a.zip`. Unutarnje putanje u kodu nemaju vodeći ni završni `/` (korijen je `""`).
- **Original se nikad ne kvari** (spec §8):
  - svako pisanje ide u `.<ime>.fxfm-<pid>-<n>.tmp` u **istoj mapi**, zatim `fsync` i `rename` preko originala;
  - prava originala se kopiraju na novu datoteku;
  - kod greške ili Cancela temp se briše, a original ostaje bajt-identičan.
- **Ugniježđene arhive** (arhiva u arhivi) nisu u planu 2. Enter na njih, F3 i F4 rade nad datotekom kao nad običnom datotekom.
- **Kriptirano:**
  - zip s lozinkom se čita (pita lozinku);
  - pisanje u arhivu u kojoj je ijedan unos kriptiran odbija se porukom `Writing into encrypted archives is not supported`;
  - kriptirani 7z daje `Encrypted 7z archives are not supported`.
- **`.gz`** ima točno jednu datoteku. Ime joj se uzima iz gzip headera, a ako ga nema, iz imena arhive bez `.gz`. Dozvoljeni su samo F3, F4, F5 van i upload back. Za F7, F8, F2, F6 i F5 unutra poruka je `Not possible in a .gz file (it holds exactly one file)`.
- **Privremene datoteke** su u `dirs::cache_dir()/fxfm/tmp/<pid>/`:
  - `stage-<n>/` za operacije, briše se po završetku operacije;
  - `view-<n>/` za F3 i Enter na datoteci, briše se pri izlasku;
  - `edit-<n>/` za F4, briše se pri izlasku samo ako nema neučitanih promjena (vidi Task 8).
- **Brisanje u arhivi** je uvijek trajno (nema Trasha). Dijalog to kaže: `Delete N file(s) from archive? This cannot be undone.`.

## Review Focus

1. **Prekid usred pisanja u arhivu** (Cancel, pun disk, greška čitanja izvora): original ostaje bajt-identičan, a u mapi ne ostaje `.fxfm-*.tmp`. Testovi u Tasku 3: `rewrite_cancel_leaves_original_identical` i `rewrite_source_error_leaves_original_identical`.
2. **Zlonamjerna imena u arhivi** (`../../x`, `/etc/x`, `a/../../b`, `C:\x`, duplikati): raspakiravanje nikad ne piše izvan odredišta. Takav unos javlja se kroz `obs.error` kao `unsafe path in archive: <ime>`, a listing ga ne prikazuje. Testovi: Task 2 `extract_refuses_path_traversal` i Task 1 `index_skips_unsafe_names`.
3. **Velika tgz arhiva** (100 000 unosa): indeks se čita jednom i kešira po (putanja, veličina, mtime). Ulazak u podmape ne raspakirava ponovno. Testovi u Tasku 1: `index_cache_reused_until_file_changes` i `list_inner_100k_entries_is_fast` (< 1 s u release modu, `#[ignore]` u debugu kao u planu 1).
4. **F4 izmjene se ne smiju izgubiti** (spec anti-cilj):
   - editor sprema dvaput u 200 ms → pita se jednom;
   - korisnik kaže No → kod sljedećeg spremanja pita se opet;
   - izlaz uz neučitane promjene → kopija ostaje na disku, a pri sljedećem pokretanju javlja se gdje je.

   Testovi u Tasku 8: `debounce_coalesces_saves`, `unchanged_content_is_not_reported`, `pending_edits_survive_cleanup`.
5. **Arhive bez dir unosa i s čudnim imenima**:
   - zip bez `dir/` unosa → mape se sintetiziraju;
   - CP437 imena iz Windows zipa se dekodiraju;
   - NFD imena s Maca se normaliziraju u NFC za usporedbu;
   - `__MACOSX/` i `._*` su vidljivi, ali sivi kao skriveni.

   Testovi u Tasku 1: `implicit_dirs_synthesized`, `zip_cp437_names_decode` i `appledouble_entries_are_hidden`.

---

## File Structure

```
crates/core/Cargo.toml                 + zip, tar, flate2, sevenz-rust2
crates/core/src/lib.rs                 + pub mod archive; pub mod vfs;
crates/core/src/archive/mod.rs         ArchiveKind, ArchiveLoc, split_location, ArchiveError, IndexEntry, ArchiveIndex, IndexCache, list_inner
crates/core/src/archive/read.rs        read_index per format, extract
crates/core/src/archive/write.rs       Edit, rewrite (zip/tar/tgz/gz/7z)
crates/core/src/archive/testutil.rs    #[cfg(test)] builders: make_zip/make_tar/make_tgz/make_gz/make_7z
crates/core/src/vfs.rs                 Loc, parse, list/nearest/dir_size/mkdir/rename/delete/copy/move preko lokalnog ili arhive; Temp
crates/core/src/config.rs              restore_path: lokacija u arhivi → mapa koja sadrži arhivu
src-tauri/Cargo.toml                   + notify 8
src-tauri/src/state.rs                 + vfs: Arc<vfs::Ctx>, edits: EditWatch
src-tauri/src/commands.rs              komande preko vfs; set_archive_password; materialize; upload_back; pending edits u Bootstrap
src-tauri/src/jobs.rs                  JobRequest → vfs; + JobRequest::Upload
src-tauri/src/editwatch.rs             F4 iz arhive: praćenje, debounce, edit://changed, pending manifest
ui/lib.mjs                             archiveKindOf, isArchiveLocation, splitArchive (čisto, po imenu)
ui/panel.js                            Enter ulazi u arhivu
ui/app.js                              lozinka, edit://changed upit, pending edits poruka, Shift+F4/cmdline u arhivi
ui/dialogs.js                          tekst brisanja iz arhive, greške passwordRequired
README.md                              odjeljak "Arhive"
```

## Zajednički tipovi (novi)

```rust
// archive/mod.rs
pub enum ArchiveKind { Zip, Tar, TarGz, Gz, SevenZ }          // Serialize camelCase
pub struct ArchiveLoc { pub archive: PathBuf, pub kind: ArchiveKind, pub inner: String }
pub struct IndexEntry { pub path: String /*NFC, bez vodećeg/završnog '/'*/, pub is_dir: bool,
    pub is_symlink: bool, pub size: u64, pub mtime: i64, pub mode: u32, pub encrypted: bool }
pub struct ArchiveIndex { pub entries: Vec<IndexEntry> /*sortirano po path, uključuje sintetizirane mape*/ }
#[derive(Error)] pub enum ArchiveError {
    #[error("password required")] PasswordRequired,
    #[error("wrong password")] WrongPassword,
    #[error("{0}")] Unsupported(String),          // gz ograničenja, kriptirano, 7z AES
    #[error("not found in archive: {0}")] NotFound(String),
    #[error("already exists in archive: {0}")] Exists(String),
    #[error("corrupt archive: {0}")] Corrupt(String),
    #[error("{0}")] Io(String),
    #[error("operation cancelled")] Cancelled,
}
```

`ArchiveError` se serializira kao `{kind, message}` (kindovi: `passwordRequired`, `wrongPassword`, `unsupported`, `notFound`, `exists`, `corrupt`, `io`, `cancelled`), po uzoru na `OpError`. `vfs` funkcije koje vraćaju `OpError` mapiraju `ArchiveError` ovako:
- `PasswordRequired` → `OpError::Io("password required: <archive>")`. UI prepoznaje prefiks `password required: ` i pita za lozinku.
- `WrongPassword` → `OpError::Io("wrong password: <archive>")`.
- Ostalo → `OpError::Io(e.to_string())`, a `Cancelled` → `OpError::Cancelled`.

---

### Task 1: Prepoznavanje lokacije, indeks i listing arhive

**Files:** Create `crates/core/src/archive/{mod.rs,read.rs,testutil.rs}`; Modify `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/config.rs`

**Interfaces:**
- Consumes: `listing::{Entry, ListError, sort_entries}`.
- Produces:
  - `ArchiveKind::from_name(name: &str) -> Option<ArchiveKind>`;
  - `pub fn split_location(loc: &str) -> Option<ArchiveLoc>`:
    - hoda komponentama od korijena;
    - prva komponenta koja postoji kao **regularna datoteka** (`symlink_metadata` prati link samo ako je meta datoteka) i ima `ArchiveKind` je arhiva;
    - ostatak je `inner`;
    - lokalna mapa koja se zove `x.zip` nije arhiva.
  - `read::read_index(path: &Path, kind: ArchiveKind) -> Result<ArchiveIndex, ArchiveError>`;
  - `pub struct IndexCache` (`Mutex<HashMap<PathBuf, (u64 size, i64 mtime_ns, Arc<ArchiveIndex>)>>`, najviše 8 arhiva, LRU izbacivanje) s metodama `get(&self, path, kind) -> Result<Arc<ArchiveIndex>, ArchiveError>` i `invalidate(&self, path)`;
  - `pub fn list_inner(index: &ArchiveIndex, inner: &str) -> Result<Vec<Entry>, ListError>`:
    - djeca direktno ispod `inner`, sortirana sa `sort_entries`;
    - `hidden` je istina za `.*`, `__MACOSX` i `._*`;
    - `inner` koji ne postoji daje `NotFound(<loc>)`, a datoteka `NotADirectory`.
  - `config::restore_path`: kad je predak postojeća regularna datoteka s `ArchiveKind`, vraća njezinu roditeljsku mapu (spec §4: "panel se otvara na lokalnoj mapi iz koje se išlo"). Ostala pravila ostaju.

Pravila indeksa:
- Imena se normaliziraju u NFC.
- `\` se ne tretira kao separator.
- Unosi s `..` komponentom, apsolutnom putanjom ili `X:` prefiksom se preskaču uz `log::warn!`.
- Mape koje se pojavljuju samo kao prefiks sintetiziraju se s `mtime` najnovijeg djeteta i `mode` `0o40755`.
- Duplikati: vrijedi zadnji (tar semantika).
- Kod zipa se `encrypted` čita iz flagova unosa (ime je čitljivo i bez lozinke).
- Kod `Gz` se veličina čita iz ISIZE trailera (mod 2³²; za > 4 GiB je informativna).

- [ ] **Step 1: testovi** (`archive/mod.rs` i `read.rs`, s builderima iz `testutil.rs`; builderi koriste iste crateove za pisanje):
  - `kind_from_name`:
    - `a.ZIP` → `Zip`;
    - `x.tar.gz` i `x.tgz` → `TarGz`;
    - `x.gz` → `Gz`;
    - `x.7z` → `SevenZ`;
    - `x.rar` i `x.bz2` → `None`.
  - `split_location`:
    - za `<t>/a.zip/src/lib` daje `inner == "src/lib"`, a za `<t>/a.zip` daje `inner == ""`;
    - mapa `<t>/d.zip/` daje `None`;
    - `<t>/nema.zip/x` (datoteka ne postoji) daje `None`.
  - `implicit_dirs_synthesized`: zip s jedinim unosom `a/b/c.txt` daje listing korijena `[a]`, a listing `a` daje `[b]`.
  - `index_skips_unsafe_names`: tar s `../evil`, `/abs` i `ok.txt` daje listing korijena koji sadrži samo `ok.txt`.
  - `zip_cp437_names_decode`: zip bez UTF-8 flaga s bajtom `0x87` daje ime koje sadrži `ç`.
  - `appledouble_entries_are_hidden`: `__MACOSX/` i `._a` imaju `hidden == true`.
  - `gz_single_entry`: gzip s headerom imena `notes.txt` daje točno jedan unos `notes.txt` s ispravnom veličinom; bez imena u headeru, `report.csv.gz` daje `report.csv`.
  - `zip_encrypted_flag`: AES zip ima `encrypted == true`, a listing radi bez lozinke.
  - `seven_z_lists`: 7z s `d/x.txt` daje listing korijena `[d]`.
  - `index_cache_reused_until_file_changes`:
    - dva `get` daju isti `Arc` (`Arc::ptr_eq`);
    - nakon prepisivanja datoteke (druga veličina) dobiva se novi `Arc`.
  - `list_inner_100k_entries_is_fast` (`#[ignore]`): tar sa 100 000 datoteka u 100 mapa; `get` + `list_inner` za jednu mapu traje < 1 s u release modu.
  - `restore_path_archive_goes_to_containing_folder`: `<t>/a.zip/src` daje `<t>`.
  - Lomljena arhiva (prvih 100 bajtova zipa) daje `ArchiveError::Corrupt`.
- [ ] **Step 2:** `cargo add -p fxfm-core zip@9 --no-default-features -F deflate,deflate64,aes-crypto,time && cargo add -p fxfm-core tar@0.4 flate2@1 && cargo add -p fxfm-core sevenz-rust2@0.25 -F compress` (bez `aes256`). Ako neki feature ne postoji pod tim imenom, odaberi najbliži iz `cargo info` i zapiši odluku u izvještaj.
- [ ] **Step 3:** Run `cargo test -p fxfm-core archive`. Expected: FAIL.
- [ ] **Step 4:** Implementiraj.
- [ ] **Step 5:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 6:** Commit `feat(core): archive locations, index and listing`.

### Task 2: Raspakiravanje

**Files:** Modify `crates/core/src/archive/read.rs`, `archive/mod.rs`

**Interfaces:**
- Consumes: `ArchiveIndex` i `IndexCache` (Task 1), `ops::{OpObserver, Progress, ErrorAnswer}`.
- Produces: `pub fn extract(path: &Path, kind: ArchiveKind, items: &[String], dest: &Path, password: Option<&str>, obs: &dyn OpObserver) -> Result<u64 /*broj datoteka*/, ArchiveError>`.
  - `items` su unutarnje putanje; mapa povlači cijelo podstablo.
  - Struktura se zadržava **relativno na roditelja stavke**: `items = ["src/lib"]` piše `dest/lib/...`.
  - Postavlja `mtime` i prava (`mode & 0o7777`; bez setuid/setgid).
  - Simlink unosi se pišu kao simlinkovi, ali samo ako cilj nakon razrješenja ostaje unutar `dest`. Inače ide `obs.error("unsafe link in archive: <ime>")`.
  - Progress je u bajtovima nekomprimiranog sadržaja; `obs.is_cancelled()` se provjerava svakih 1 MiB.
  - `dest` mora postojati i biti prazan (staging), pa nema konflikata.
  - Lozinka (zip):
    - kriptirani unos bez lozinke daje `PasswordRequired`;
    - kriva lozinka daje `WrongPassword`.

    Provjera ide na prvom kriptiranom unosu, prije pisanja bilo čega.
  - `TarGz`, `Tar` i `Gz` čitaju arhivu u jednom prolazu i preskaču unose izvan `items`.
  - `SevenZ` koristi `sevenz-rust2` dekoder s filterom po imenu.

- [ ] **Step 1: testovi**:
  - `extract_subtree_relative_to_parent`: `src/lib/a.rs` i `src/lib/m/b.rs` s `items=["src/lib"]` daju `dest/lib/a.rs` i `dest/lib/m/b.rs`, sa sadržajem i `mtime` kao u arhivi.
  - `extract_refuses_path_traversal`: ručno složen tar s unosom `../evil` i simlinkom `l -> ../../etc` s `items=[""]` ne stvara `dest/../evil` i ne stvara link `l`. `obs.error` je pozvan barem jednom, a ostale datoteke postoje.
  - `extract_all_formats`: za Zip, Tar, TarGz, Gz i SevenZ isti mali sadržaj (`a.txt`, `d/b.txt`; za Gz samo jedna datoteka) raspakira se bajt-identično.
  - `extract_password`:
    - AES zip bez lozinke daje `PasswordRequired`, s krivom `WrongPassword`, a s ispravnom izlazi sadržaj;
    - kod greške je `dest` prazan.
  - `extract_cancel_stops`: observer koji vraća `is_cancelled` nakon prvog progressa daje `Err(Cancelled)` za arhivu s datotekom od 5 MiB.
- [ ] **Step 2:** Run `cargo test -p fxfm-core archive::read`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): archive extraction with path-safety and passwords`.

### Task 3: Prepisivanje arhive (dodaj, obriši, preimenuj, nova mapa)

**Files:** Create `crates/core/src/archive/write.rs`

**Interfaces:**
- Consumes: Task 1 i 2.
- Produces:
  - `pub enum Edit { Add { src: PathBuf, inner: String }, Delete { inner: String }, Rename { from: String, to: String }, Mkdir { inner: String } }`;
  - `pub fn rewrite(path: &Path, kind: ArchiveKind, edits: &[Edit], obs: &dyn OpObserver) -> Result<(), ArchiveError>`.

Pravila za `rewrite`:
- `Add` s lokalnom mapom dodaje cijelo podstablo pod `inner`. Postojeći unos istog imena (ili podstablo) se zamjenjuje; konflikte pita `vfs` prije poziva, ne `rewrite`.
- `Delete` i `Rename` nad mapom djeluju na cijelo podstablo.
- `Rename` na postojeće ime daje `Exists`, a nepostojeći `from` daje `NotFound`.
- `Mkdir` na postojeće ime daje `Exists`.
- Edits se primjenjuju redom na virtualni popis, a zatim se arhiva piše jednom.
- Kriptirani unos bilo gdje u arhivi daje `Unsupported("Writing into encrypted archives is not supported")` prije pisanja.
- `Gz` dopušta samo:
  - jedan `Add` čiji je `inner` jednak postojećem imenu (zamjena sadržaja);
  - jedan `Rename` (mijenja ime u gzip headeru).

  Ostalo daje `Unsupported("Not possible in a .gz file (it holds exactly one file)")`.
- **Zip**: nepromijenjeni unosi se kopiraju raw, bez ponovnog komprimiranja (`ZipWriter::raw_copy_file`, s preimenovanjem preko `raw_copy_file_rename`). Novi unosi su Deflate. Zadržavaju se mtime i Unix prava.
- **Tar i TarGz**: piše se nova arhiva, nepromijenjeni unosi se streamaju iz stare, a TarGz koristi gzip default level.
- **SevenZ**: raspakiravanje u staging pod temp rootom, pa `sevenz-rust2` kompresija cijelog stabla (7z se ne može raw-kopirati). Zapiši to u komentar i u README jer je sporije.
- Atomičnost je iz Global Constraints:
  - temp u istoj mapi;
  - `sync_all`;
  - `set_permissions` s prava originala;
  - `rename`;
  - `fsync` roditeljske mape;
  - na kraju `IndexCache::invalidate`.

  Zato `rewrite` prima i `cache: &IndexCache` kao zadnji argument. Konačni potpis: `rewrite(path, kind, edits, cache, obs)`.
- Progress je u bajtovima napisanog sadržaja. Kod Cancela se temp briše, a vraća se `Err(Cancelled)`.

- [ ] **Step 1: testovi** (svi formati gdje ima smisla; nakon svakog `rewrite` arhiva se ponovno čita i provjerava sa `read_index` i `extract`):
  - `add_file_and_dir`: dodaj `x.txt` u `""` i lokalnu mapu `m/` (s `m/y.txt`) u `src`. Indeks tada sadrži `x.txt`, `src/m/y.txt` i sve stare unose bajt-identično.
  - `add_replaces_existing`: `Add` na `a.txt` mijenja sadržaj, a broj unosa ostaje isti.
  - `delete_dir_subtree`, `rename_dir_subtree` (`d` → `e` seli `d/b.txt` u `e/b.txt`), `rename_to_existing_is_exists`, `mkdir_creates_empty_dir`, `mkdir_existing_is_exists`.
  - `gz_rules`:
    - zamjena sadržaja radi;
    - `Rename` mijenja ime koje `read_index` vraća;
    - `Mkdir` i `Delete` daju `Unsupported`.
  - `encrypted_zip_write_refused`: `Unsupported`, a datoteka je bajt-identična.
  - `rewrite_cancel_leaves_original_identical`: cancel nakon prvog progressa daje `Err(Cancelled)`. Original je bajt-identičan (usporedi `fs::read`), a u mapi nema datoteke koja počinje s `.` i završava s `.tmp`.
  - `rewrite_source_error_leaves_original_identical`: `Add` s nepostojećim `src` daje `Err(Io)`, a original je identičan i bez tempa.
  - `rewrite_keeps_permissions`: original `0o640` je i nakon pisanja `0o640`.
  - `zip_raw_copy_keeps_compressed_bytes`: nepromijenjeni unos ima isti `compressed_size` i `crc32` prije i poslije.
- [ ] **Step 2:** Run `cargo test -p fxfm-core archive::write`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): atomic archive rewrite (add, delete, rename, mkdir)`.

### Task 4: VFS routing — listing, navigacija i operacije bez prijenosa

**Files:** Create `crates/core/src/vfs.rs`; Modify `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: Task 1–3, `listing::list_dir`, `ops::{mkdir, rename, delete, dir_size}`, `config::restore_path`.
- Produces:
  - `pub enum Loc { Local(PathBuf), Archive(ArchiveLoc) }` i `pub fn parse(loc: &str) -> Loc`. `split_location` daje `Archive`, inače `Local`.
  - `pub struct Ctx` sadrži:
    - `pub cache: IndexCache`;
    - `pub temp_root: PathBuf`;
    - `passwords: Mutex<HashMap<PathBuf, String>>`;
    - brojač `n: AtomicU64`.

    Metode:
    - `Ctx::new(temp_root: PathBuf)`;
    - `set_password(&self, archive: &Path, pw: String)`;
    - `password(&self, archive: &Path) -> Option<String>`;
    - `new_temp_dir(&self, prefix: &str) -> io::Result<PathBuf>` stvara `temp_root/<prefix>-<n>`;
    - `cleanup_dir(&self, dir: &Path)`.
  - Funkcije:
    - `pub fn list(ctx: &Ctx, loc: &str) -> Result<Vec<Entry>, ListError>`. Za arhivu je to `cache.get` + `list_inner`. `ArchiveError::Corrupt` i ostale daju `ListError::Io(<loc>)` uz `log::warn!` s porukom.
    - `pub fn nearest(ctx: &Ctx, loc: &str, home: &Path) -> String`. Za arhivu: najbliža postojeća unutarnja mapa, pa korijen arhive. Ako arhiva više ne postoji, vraća `restore_path`.
    - `pub fn dir_size(ctx: &Ctx, loc: &str, cancel: &AtomicBool) -> io::Result<(u64, u64)>`. Za arhivu se zbraja iz indeksa.
    - `pub fn mkdir(ctx: &Ctx, parent: &str, name: &str) -> Result<String, String>`. Arhiva: `rewrite([Mkdir])` s `NullObserver`.
    - `pub fn rename(ctx: &Ctx, loc: &str, new_name: &str) -> Result<String, String>`. Arhiva: `rewrite([Rename])`. Za oba vrijedi pravilo imena iz privatnog `ops::valid_name`; učini ga `pub` (bez promjene ponašanja). Vraća se nova lokacija kao `String`.
    - `pub fn delete(ctx: &Ctx, locs: &[String], to_trash: bool, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`. Sve lokacije su iz iste mape (UI to garantira). Arhiva: `to_trash` se ignorira i radi se jedan `rewrite` sa svim `Delete`. `OpSummary.done` je broj obrisanih ne-mapa unosa.
  - `pub(crate) struct NullObserver` (overwrite, skip na grešku, nikad cancel) za kratke operacije.
  - `pub fn local_dir_for(loc: &str) -> PathBuf`: za arhivu je to mapa koja sadrži arhivu, za lokalno sama lokacija. Koriste ga Terminal i naredbeni redak.

- [ ] **Step 1: testovi**:
  - `list_routes_local_and_archive`: lokalna mapa s `a.zip` daje `list("<t>")`, koji sadrži `a.zip`, i `list("<t>/a.zip/d")` s `b.txt`.
  - `nearest_inside_archive`:
    - `<t>/a.zip/d/nema` daje `<t>/a.zip/d`;
    - `<t>/a.zip/nema/x` daje `<t>/a.zip`;
    - `<t>/obrisano.zip/x` daje `<t>`.
  - `mkdir_rename_delete_in_archive`: `mkdir("<t>/a.zip", "n")` vraća `<t>/a.zip/n`; zatim `rename("<t>/a.zip/n", "m")`, pa `delete(["<t>/a.zip/m", "<t>/a.zip/a.txt"])`. Listing poslije odgovara očekivanom, a `done == 1`.
  - `gz_mkdir_is_refused_with_message`: greška sadrži `Not possible in a .gz file`.
  - `dir_size_in_archive`: zbroj veličina i broj datoteka podstabla.
  - `local_paths_unchanged`: `mkdir`, `rename` i `delete` na lokalnim lokacijama daju isti rezultat kao direktni `ops::*`.
  - `local_dir_for_archive`: `<t>/a.zip/d` daje `<t>`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core vfs`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): vfs routing for listing, mkdir, rename, delete`.

### Task 5: VFS copy i move preko staginga

**Files:** Modify `crates/core/src/vfs.rs`

**Interfaces:**
- Consumes: Task 4, `ops::{copy, move_, resolve_dest, Dest, ConflictInfo, ConflictAnswer}`.
- Produces:
  - `pub fn copy(ctx: &Ctx, sources: &[String], dest: &str, cwd: &str, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`;
  - `pub fn move_(ctx: &Ctx, sources: &[String], dest: &str, cwd: &str, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`.

Algoritam:
- Svi izvori su iz iste lokacije. Odredište je `dest`, relativno na `cwd` ako nije apsolutno, i parsira se s `parse`. `IntoDir` i `AsFile` se određuju kao u `ops::resolve_dest`, ali "je li mapa" za arhivu se pita indeks.
1. **Lokalno → lokalno**: poziva se direktno `ops::copy` ili `ops::move_`, bez promjene ponašanja.
2. **Arhiva → lokalno**:
   - `extract` izvora u `stage-<n>`;
   - zatim `ops::copy` iz staginga na lokalno odredište s pravim `obs`, pa konflikti, Rename i Overwrite older rade kao lokalno;
   - za `AsFile` se staged datoteka kopira pod novo ime;
   - `move_` nakon toga radi jedan `rewrite([Delete…])` samo za izvore koji su stvarno kopirani (nisu preskočeni). To se pamti preko observera koji broji skip po imenu na vrhu, kao wrapper oko `obs`.
3. **Lokalno → arhiva**:
   - za svaki izvor najvišeg nivoa provjeri postoji li `dest_inner/<ime>` u indeksu;
   - ako postoji, zovi `obs.conflict` s `ConflictInfo` (`target_size` i `target_mtime` iz indeksa). Skip ga izostavlja, Rename mijenja ime, Overwrite older uspoređuje mtime, a `*All` odgovori se pamte;
   - zatim jedan `rewrite([Add…])`;
   - `move_` briše lokalne izvore (`ops::delete` trajno, `NullObserver`) **tek nakon** uspješnog `rewrite`.

   Konflikt je na razini stavke najvišeg nivoa (mapa koja postoji = jedno pitanje), što je pojednostavljenje u odnosu na lokalni per-file. Zapiši to u komentar.
4. **Arhiva → arhiva** (ista ili druga): `extract` u staging, pa korak 3 sa staging putanjama kao izvorima. Za `move_` unutar **iste** arhive koristi se `rewrite([Rename…])` umjesto extract+add.
5. **Into itself**:
   - lokacija odredišta koja je jednaka izvoru ili ispod njega daje `OpError::IntoItself`;
   - odredište jednako roditelju izvora kod `move_` daje `SameLocation`;
   - isto vrijedi za unutarnje putanje.
6. Staging se uvijek briše na kraju (i kod greške).
7. Progress:
   - faza raspakiravanja javlja `current` kao `<ime> (extracting)`;
   - faza upisa kao `<ime> (packing)`;
   - `total_bytes` je suma iz indeksa, odnosno lokalnih veličina.

- [ ] **Step 1: testovi** (s novim test-observerom `RecordingObserver` u `vfs.rs` testovima, po uzoru na `TestObserver` u `ops.rs`, koji vraća zadane odgovore i bilježi pozive):
  - `copy_archive_to_local`: `<t>/a.zip/d` u `<t>/out` daje `<t>/out/d/b.txt`, a staging mapa više ne postoji.
  - `copy_archive_to_local_conflict_uses_ops_rules`: `<t>/out/a.txt` već postoji, a odgovor je Skip. Sadržaj ostaje nepromijenjen i `skipped == 1`.
  - `copy_local_into_archive_with_conflict`:
    - `x.txt` je već u arhivi i odgovor je `Rename{newName:"x (2).txt"}`;
    - indeks tada ima oba, a stari je bajt-identičan;
    - `ConflictInfo.target_size` odgovara indeksu.
  - `move_local_into_archive_deletes_source_after_success`: lokalni izvor je obrisan, a u arhivi je.
  - `move_local_into_archive_keeps_source_on_failure`: arhiva je zaključana za pisanje (`chmod 0o500` na mapi), pa je rezultat `Err`. Lokalni izvor postoji, a original arhive je identičan.
  - `move_archive_to_local_skipped_not_deleted`: Skip na jednom od dva izvora ostavlja taj u arhivi, a drugi je obrisan iz arhive.
  - `move_within_same_archive_is_rename`: `d/b.txt` u `e/` daje `e/b.txt`, a `d/b.txt` više ne postoji.
  - `copy_between_archives`: zip → tgz.
  - `copy_into_itself_in_archive_refused`: `<t>/a.zip/d` u `<t>/a.zip/d/x` daje `IntoItself`.
  - `password_required_maps_to_io_prefix`: kriptirani izvor bez lozinke daje `OpError::Io`, koji počinje s `password required: `. Nakon `ctx.set_password` kopija uspije.
- [ ] **Step 2:** Run `cargo test -p fxfm-core vfs`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): copy and move between local folders and archives`.

### Task 6: Tauri backend preko vfs-a

**Files:** Modify `src-tauri/src/{state.rs,commands.rs,jobs.rs,main.rs}`

**Interfaces:**
- Consumes: `vfs::*` (Task 4 i 5).
- Produces:
  - `AppState.vfs: Arc<vfs::Ctx>`, gdje je `temp_root = dirs::cache_dir()/fxfm/tmp/<pid>`. Kod starta se brišu `tmp/<pid>` mape čiji proces ne postoji (`libc::kill(pid, 0)` daje `ESRCH`), osim onih koje imaju `pending-edits.json` (Task 8). Kod `RunEvent::Exit` se briše vlastiti `temp_root` (Task 8 to sužava).
  - Komande `list_dir`, `nearest_existing`, `dir_size`, `mkdir`, `rename` zovu `vfs::*`. Imena i argumenti ostaju isti, pa se UI ne mijenja.
  - `run_job` zove `vfs::copy`, `vfs::move_` i `vfs::delete`. Novi `JobRequest::Upload { local: String, target: String }` radi `vfs::copy([local], target)` s observerom koji na konflikt uvijek odgovara `Overwrite` (koristi ga Task 8). Serde oblik je `{"kind":"upload","local":…,"target":…}`, kao postojeći varijanti.
  - Nova komanda `set_archive_password(archive: String, password: String)`. `archive` je putanja do datoteke arhive, koju UI dobije iz poruke greške.
  - Nova komanda `materialize(location: String) -> Result<String, String>`:
    - za lokalnu lokaciju vraća je nepromijenjenu;
    - za datoteku u arhivi radi `extract` u `view-<n>/` i vraća lokalnu putanju;
    - `PasswordRequired` daje `Err("password required: <archive>")`.
  - `open_viewer(path)` i `open_default(path)` najprije pozovu `materialize`. Naslov viewera je puna lokacija u arhivi.
  - `run_in_terminal(dir, …)` koristi `vfs::local_dir_for(dir)`.
  - `edit_new` u arhivi daje `Err("Shift+F4 is not available inside archives")`.
  - Postojeća Rust logika `nearest_existing` za `://` ostaje.

- [ ] **Step 1: testovi** (`jobs.rs` i `commands.rs` unit, bez Taurija gdje je moguće):
  - `job_request_upload_serde`: `{"kind":"upload","local":"/t/x","target":"/t/a.zip/x"}` se parsira.
  - `run_job_routes_archive_copy`: `run_job` s Copy iz arhive u temp mapu kopira datoteku. Koristi mali test-observer u `jobs.rs` testovima (`TauriObserver` treba AppHandle).
  - `upload_overwrites_without_asking`: observer koji panica na `conflict` ne biva pozvan.
  - `stale_temp_dirs_removed_except_pending`: `tmp/999999/` se briše, a `tmp/999998/` s `pending-edits.json` ostaje.
- [ ] **Step 2:** Run `cargo test --workspace`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): commands and jobs go through vfs (archives)`.

### Task 7: UI — ulazak u arhivu, lozinka, brisanje, poruke

**Files:** Modify `ui/lib.mjs`, `ui/lib.test.mjs`, `ui/panel.js`, `ui/app.js`, `ui/dialogs.js`

**Interfaces:**
- Consumes: komande iz Taska 6.
- Produces:
  - `lib.mjs`:
    - `archiveKindOf(name) -> "zip"|"tar"|"tgz"|"gz"|"7z"|null`, s istim pravilima kao Rust `from_name`;
    - `archiveOf(location) -> string|null`, prefiks lokacije do prve komponente s `archiveKindOf`. Čisto po imenu i koristi se samo za UI odluke; backend je autoritet.
    - `passwordArchive(message) -> string|null`, koji iz `"password required: /x/a.zip"` ili `"wrong password: …"` vraća putanju.
  - `panel.enter()`:
    - datoteka s `archiveKindOf` kad panel **nije** već u arhivi radi `go(joinLocation(loc, name))`;
    - u arhivi ugniježđena arhiva ide na `openDefault`, a ostale datoteke na `openDefault` (backend materijalizira).
  - Ako `go` padne s greškom koja sadrži `password required` (listing ne treba lozinku, ali F3, F5 i Enter na datoteci trebaju):
    - `app.js` ima helper `withPassword(fn)`: na takvu grešku otvara prompt `Password for <ime arhive>:` (input type password, OK i Cancel), poziva `set_archive_password` i ponavlja `fn` jednom;
    - kod `wrong password` prompt se ponovi s tekstom `Wrong password for <ime>. Try again:`;
    - koriste ga F3, Enter na datoteci i job greške (`dialogs.js`: kad job završi s greškom koja je password-tip, ponudi prompt i ponovno pokreni isti `req`).
  - Brisanje: ako je `archiveOf(panel.location)`, dijalog je `Delete N file(s) from archive? This cannot be undone.`, a šalje se `toTrash: false`. Shift+F8 je isto.
  - Shift+F4 u arhivi prikazuje status poruku iz backenda (bez dijaloga).
  - Prompt naredbenog retka ostaje `<location>>`, a backend bira mapu.

- [ ] **Step 1: testovi** (`lib.test.mjs`):
  - `archiveKindOf`: `A.ZIP` → `zip`; `x.tar.gz` i `x.tgz` → `tgz`; `x.gz` → `gz`; `x.7z` → `7z`; `x.rar` i `x.bz2` → `null`.
  - `archiveOf`: `/u/a.zip/src` → `/u/a.zip`; `/u/a.zip` → `/u/a.zip`; `/u/src` → `null`.
  - `passwordArchive`: dva oblika poruke daju putanju, a ostale poruke `null`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): browse archives, password prompt, delete from archive`.

### Task 8: F4 iz arhive s uploadom natrag

**Files:** Create `src-tauri/src/editwatch.rs`; Modify `src-tauri/Cargo.toml`, `state.rs`, `commands.rs`, `main.rs`, `ui/app.js`

**Interfaces:**
- Consumes: `materialize` (s `edit-<n>/` umjesto `view-<n>/`), `JobRequest::Upload`.
- Produces:
  - `pub struct EditWatch` u `AppState.edits`:
    - `register(&self, location: String, local: PathBuf) -> u64`, koji pamti SHA-256 sadržaja (crate `sha2` u `src-tauri`) i počinje praćenje roditeljske mape preko `notify` 8 `RecommendedWatcher`;
    - pri svakom eventu za tu datoteku pokreće debounce od **500 ms**;
    - nakon toga ponovno računa hash, pa ako je različit od zadnjeg **potvrđenog**, emitira `edit://changed { id, location, name }` na `main`. Dok UI ne odgovori, nema novog eventa za isti `id`;
    - `confirm(&self, id, uploaded: bool)`: `uploaded=true` postavlja zadnji potvrđeni hash na trenutni, a `false` ostavlja stari, pa sljedeće spremanje opet pita;
    - `pending(&self) -> Vec<(String location, PathBuf local)>` vraća one čiji je trenutni hash različit od potvrđenog.
  - `edit_file(path)` za lokaciju u arhivi radi `materialize` u `edit-<n>/`, `register`, pa otvara editor. Lokalna datoteka ide kao dosad.
  - Komanda `upload_back(id) -> u64`: pokreće `JobRequest::Upload { local, target: location }` i vraća job id (UI ga prati postojećim progress dijalogom). Po uspjehu UI zove `edit_confirm(id, true)`.
  - Komanda `edit_confirm(id: u64, uploaded: bool)`.
  - **Izlaz:** kod `RunEvent::Exit`, ako `pending()` nije prazan:
    - u `temp_root` se zapisuje `pending-edits.json` (`[{location, local}]`);
    - `edit-*` mape s pending datotekama se **ne** brišu, a sve ostalo se briše.
  - **Start:**
    - `Bootstrap.pendingEdits: Vec<{location, local}>` skuplja `pending-edits.json` iz mapa mrtvih procesa;
    - UI prikazuje `Edited copies from a previous session were not uploaded:` s popisom, uz gumbe `Show in Finder` (`open -R <local>`) i OK;
    - nakon OK zove `dismiss_pending_edits()`, koja briše te mape.
  - **UI** na `edit://changed` prikazuje dijalog `"<name>" was changed. Upload it back into the archive?` s gumbima Yes i No:
    - Yes radi `upload_back` (a `edit_confirm(id,true)` nakon što job uspije);
    - No radi `edit_confirm(id,false)`.

- [ ] **Step 1: testovi** (`editwatch.rs`, logika odvojena od Taurija preko closurea `emit: Fn(u64, &str)`):
  - `debounce_coalesces_saves`: dva zapisa u razmaku od 100 ms daju točno jedan emit u roku od 1,5 s.
  - `unchanged_content_is_not_reported`: zapis istog sadržaja (touch) ne daje emit.
  - `declined_change_asks_again`: nakon `confirm(id,false)` sljedeća promjena opet daje emit.
  - `pending_edits_survive_cleanup`: neučitana promjena nakon `exit_cleanup(temp_root)` ostavlja datoteku i `pending-edits.json`. Učitana promjena znači da je mapa obrisana.
- [ ] **Step 2:** `cargo add -p fxfm notify@8 sha2@0.10`. Run `cargo test --workspace`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 5:** Commit `feat: edit files inside archives with upload back`.

### Task 9: README, release build i ručna provjera

**Files:** Modify `README.md`

- [ ] **Step 1:** README odjeljak **Arhive** (hrvatski, kao ostatak README-a):
  - formati;
  - što radi, a što ne (gz pravila, kriptirano, ugniježđeno, 7z se pri pisanju u potpunosti prepakira);
  - gdje su privremene datoteke;
  - što znači poruka o neučitanim izmjenama.
- [ ] **Step 2:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 3:** Run `cargo tauri build --target universal-apple-darwin && lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm`. Expected: `x86_64 arm64`.
- [ ] **Step 4: ručna provjera** (popis za korisnika, ako GUI nije dostupan):
  1. Enter na zip, tgz, 7z i gz; `[..]` vraća na arhivu s kursorom na njoj.
  2. F5 iz arhive u lokalnu mapu i obrnuto, s konfliktom (Rename).
  3. F6 unutar iste arhive, F7, F2 i F8 (tekst "from archive").
  4. F3 na `.txt` i na slici u arhivi.
  5. F4 na `.php` u zipu: promijeni, spremi, Yes; zatvori, ponovno otvori zip i provjeri promjenu. Zatim promijeni, No, i promijeni opet (pita ponovno).
  6. AES zip: F3 pita lozinku; kriva lozinka pita ponovno.
  7. Cmd+Q uz neučitanu F4 promjenu: pri sljedećem pokretanju javlja se poruka s putanjom.
- [ ] **Step 5:** Commit `docs: archives in README`.

---

## Nakon plana 2

- **Plan 3 (serveri)**: `SftpFs` i `FtpFs`, kao treća grana u `vfs::Loc`. Staging model iz Taska 5 vrijedi i za server: preuzmi u staging, pa upload. Tada se procjenjuje uvodi li se trait.
- **Kandidati za kasnije:**
  - pakiranje novih arhiva (Alt+F5), jer `rewrite` na praznoj arhivi to već skoro radi;
  - ugniježđene arhive;
  - bz2 i xz tar;
  - per-file konflikti pri kopiranju u arhivu.
