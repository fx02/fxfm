# fxfm

fxfm je dvopanelni upravitelj datoteka za macOS u stilu Total Commandera (Rust + Tauri 2). Tipke su TC-ove (F3–F8, Tab, Ctrl+D…, uvijek fizički **Ctrl**, Option je Alt), meni je u macOS gornjoj traci (Files, Mark, Commands, Net, Show, Configuration, Start, Help). Radi s lokalnim diskovima, arhivama (kao mapama) i serverima (SFTP, SCP, FTP, FTPS).

Traži macOS 15 (Sequoia) ili noviji.

## Build

Potrebni su Rust (toolchain iz `rust-toolchain.toml`, s targetima `aarch64-apple-darwin` i `x86_64-apple-darwin`) i `tauri-cli` 2 (`cargo install tauri-cli --version "^2"`).

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo tauri build --target universal-apple-darwin
```

Rezultat (universal, radi na Apple Silicon i Intel Macovima):

- aplikacija: `target/universal-apple-darwin/release/bundle/macos/fxfm.app`
- disk image: `target/universal-apple-darwin/release/bundle/dmg/fxfm_0.1.0_universal.dmg`

Ako se `.dmg` korak sruši s "Finder is busy" (zaključan ekran, SSH), pokreni build s `CI=true`: preskače se Finderovo uređivanje izgleda DMG-a.

`tauri.conf.json` postavlja `signingIdentity` na "fxfm local signing", pa je svaki build potpisan tim lokalnim identitetom (nalazi se u `~/.fxfm-signing/`, nije u repou). `scripts/build-macos.sh` otključa taj keychain, napravi universal build te provjeri potpis i arhitekture. Identitet je stabilan, pa macOS pamti odobren pristup mapama između buildova.

Provjera: `lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm` ispisuje `x86_64 arm64`.

Testovi: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` i `node --test ui/`.

## Gdje su podaci

Postavke su četiri JSON datoteke: `settings.json`, `session.json`, `keymap.json` i `hotlist.json`. Mogu se uređivati ručno (dok fxfm nije pokrenut).

- **Portable**: mapa `fxfm-data/` pored `fxfm.app`, ako postoji i zapisiva je.
- **Sistemski** (inače): `~/Library/Application Support/fxfm/`.

Način se mijenja u Configuration → Options → Storage (Apply). fxfm tada kopira sve postavke na novo mjesto. Kod prelaska s portable na sistemski stara mapa ostaje kao `fxfm-data.old`.

## Portable postavljanje

Napravi mapu, npr. na USB disku:

```
fxfm/
├── fxfm.app
└── fxfm-data/      (prazna mapa)
```

Kod prvog pokretanja fxfm prepozna `fxfm-data/` i sve sprema tamo; sistemska mapa se ne dira.

## Prijenos na drugi Mac

Kopiraj cijelu mapu `fxfm/` (USB, mreža, zip). macOS kopiranoj aplikaciji doda *quarantine* atribut. Zato je Gatekeeper može blokirati, a *App Translocation* je pokreće s nasumične read-only putanje, gdje se `fxfm-data/` ne vidi.

fxfm to prepozna i ponudi:

- **Fix and restart**: makne quarantine atribut i ponovno se pokrene s pravog mjesta, s postavkama iz `fxfm-data/`;
- **Continue without settings**: radi dalje, ali bez portable postavki, a sesija (paneli) se ne sprema.

Isto ručno, u Terminalu, prije prvog pokretanja:

```sh
xattr -dr com.apple.quarantine /putanja/do/fxfm/fxfm.app
```

Aplikacija je potpisana lokalnim identitetom, ali nije notarizirana (osobna upotreba). Ako je Gatekeeper ipak odbije, najjednostavnije je pokrenuti gornju `xattr -dr com.apple.quarantine …` naredbu. Druga mogućnost: pokušaj otvoriti fxfm, pa u System Settings → Privacy & Security klikni "Open Anyway" (na macOS 15 desni klik → Open više ne zaobilazi Gatekeeper).

## Arhive

Arhive se otvaraju kao mape. Podržani formati: `.zip`, `.tar`, `.tgz` / `.tar.gz`, `.gz` (samo ako sadrži točno jednu datoteku) i `.7z`. Ostale (bz2, xz, rar, tbz2, txz) Enter otvara zadanom aplikacijom.

- **Ulaz i izlaz**: Enter na arhivi ulazi u nju (arhive na serveru: vidi "Serveri"); `[..]` u korijenu arhive vraća u mapu s kursorom na arhivi. Ugniježđene arhive Enter otvara zadanom aplikacijom (ne kao mape).
- **Unutar arhive rade**: F3, F4, F5 i F6 (u arhivu i iz nje), F7, F8 i F2. Shift+F4 unutra nije dostupan.
- **F8** briše trajno: "Delete N file(s) from archive? This cannot be undone."
- **Kopiranje u arhivu** postavlja jedno pitanje o konfliktu po stavci najviše razine; prepisivanje mape spaja sadržaj u nju.
- **Lozinka**: za zip zaštićen lozinkom fxfm je pita kad se datoteka otvori ili raspakira; kriva lozinka pita ponovno.
- **F4 (uređivanje)**: datoteka se uredi kao kopija u privremenoj mapi. Nakon svakog spremanja fxfm pita `"<ime>" was changed. Upload it back into the archive?`. Ako se unos u arhivi u međuvremenu promijenio, prije prepisivanja pita još jednom.
- **Neučitane izmjene**: kopije koje nisu učitane natrag preživljavaju Cmd+Q i rušenje. Pri sljedećem pokretanju fxfm ih navodi ("Edited copies from a previous session were not uploaded:"): **Show in Finder** ih otvara, **Keep** (zadano, i Esc) ih ostavlja pa se dijalog javlja opet pri sljedećem pokretanju, a **Discard** nakon potvrde ("Move these copies to the Trash?") premješta kopije u Trash.
- **Pisanje**: svaka promjena piše privremenu arhivu pored originala, koja ga zamjenjuje tek ako je sve uspjelo. **7z** se pritom u potpunosti prepakira (sporije, treba slobodnog mjesta pored arhive, rezultat nije solid).
- **Odbijanja**: kriptirane arhive se ne mijenjaju; kriptirani 7z se ne može čitati; 7z s nesigurnim imenima unosa se ne prepisuje; tar u kojem hard link pokazuje na stavku koja se briše ili preimenuje se ne mijenja; `.gz` ima vlastita ograničenja (jedna datoteka).
- **Privremene datoteke**: `~/Library/Caches/fxfm/tmp/<pid>/`. Brišu se pri izlasku, osim neučitanih izmjena.

## Serveri

**Ctrl+F** (Net → Connect to server) otvara popis servera: stablo mapa i servera, s gumbima *New connection…* i *New URL…*. Protokoli: **SFTP**, **SCP** (zadani port 22), **FTP** i **FTPS** (21). *New URL…* spaja jednokratno, bez spremanja, npr. `sftp://korisnik@host:2222/var/www`; FTP bez korisnika se prijavljuje kao anonymous.

- **Lokacija** je `<protokol>://<ime servera>/<putanja>`, npr. `sftp://web-1/var/www/html`. Ime servera mora biti jedinstveno u cijelom popisu. Spojeni serveri se pojavljuju kao diskovi u traci iznad panela (☁️ ime), a u zaglavlju je njihova adresa. Hotlista prima iste lokacije (`cd sftp://<ime>/var/www` u naredbenom retku spaja i otvara).
- **Autentikacija**: lozinka, ključ ili ključ + passphrase. Privatni ključ se bira s diska ili uvozi (*Import…*): kopija ide u `keys/` u mapi s podacima, s pravima 0600. Opcije po serveru: lokalna mapa koja se nakon spajanja otvara u DRUGOM panelu, kodiranje imena na FTP-u (UTF-8, Windows-1250, Latin-1), stariji SSH algoritmi, FTP aktivni/pasivni način.
- **Host key**: pri prvom spajanju fxfm pokaže otisak ključa servera (kod FTPS-a certifikat); prihvaćeni se pamte u `known_hosts` u mapi s podacima. Promijenjen ključ se ne prihvaća šutke: dijalog nudi *Replace and connect* ili Cancel.
- **Lozinke** nikad ne idu u `servers.json`.
  - *Sistemski način*: macOS Keychain, servis `fxfm`, račun `<id servera>/password` ili `<id servera>/passphrase`.
  - *Portable način*: `secrets.json` u `fxfm-data/`, kriptiran **master lozinkom** (Argon2id 64 MiB, XChaCha20-Poly1305). Master lozinka se traži jednom po pokretanju, kad zatreba, i drži se samo u memoriji. Bez nje se lozinke ne spremaju (unesena a nespremljena lozinka vrijedi do izlaza, zbog ponovnog spajanja).
- **Rad**: F3, F4, F5, F6, F7, F8 i F2 rade i na serveru, kao i **chmod** (Files → Change attributes…, bez zadane tipke; samo SFTP i SCP). F8 briše trajno: "Delete N file(s) from the server? This cannot be undone.". Upload piše u privremenu `.<ime>.fxfm-<pid>-<n>.part` pored cilja pa ga zamjenjuje, pa prekid nikad ne ostavlja skraćen original; na SFTP-u i SCP-u prava postojeće datoteke ostaju (FTP i FTPS ne postavljaju prava).
- **Simbolički linkovi**: Enter na linku do mape ulazi u mapu, a F8 briše samo link. Neki FTP serveri link do mape prikazuju kao običnu mapu; tamo F8 na njemu briše sadržaj ciljne mape.
- **F4** na datoteci na serveru uređuje privremenu kopiju; nakon spremanja fxfm pita `"<ime>" was changed. Upload it back to the server?`.
- **Arhive na serveru**: Enter na arhivi preuzima je kao F4 kopiju i ulazi u nju. Izmjene unutra (F5, F8, F2…) mijenjaju kopiju, pa fxfm nudi upload natrag na server. `[..]` u korijenu arhive vraća u mapu na serveru.
- **Ctrl+Shift+F** (Net → Disconnect) zatvara vezu aktivnog panela; paneli na tom serveru idu u lokalnu mapu servera ili u home. Privremene kopije datoteka tog servera (bez neučitanih izmjena) se brišu.
- **Connection lost**: spajanje traje najviše 15 s, a prijenos ili zahtjev bez napretka 60 s; SSH šalje keepalive svakih 30 s, a FTP `NOOP` nakon 30 s neaktivnosti. Mrtva veza se javlja dijalogom `Connection to <ime> lost` s gumbima **Reconnect** (paneli ponovno listaju svoje mape) i **Close**.
- **Podaci**: `servers.json` (stablo, bez lozinki), `known_hosts`, `keys/` i (portable) `secrets.json`, u mapi s podacima.

## F3 preslušavanje zvuka

F3 na audio datoteci svira je odmah, u glavnom prozoru; fokus ostaje u panelu. Traka iznad F-tipki pokazuje ime, `m:ss / m:ss` i kodek; klik na traku skače na to mjesto. F3 na istoj datoteci ili Esc zaustavlja. Radi i u arhivama i na serverima. Formati: wav (PCM, float, A-law, μ-law, ADPCM, GSM 6.10), mp3, flac, ogg, opus, m4a, aiff.

Izlazni uređaj se bira u Options → Audio → *Output device* (zadano *System default*); promjena vrijedi odmah, i za ono što upravo svira. Ako izabrani uređaj nije spojen, svira se na zadanom, a traka to jednom javi.

## Drag & drop

- **Van**: povlačenje označenih stavki (ili retka pod pokazivačem) iz panela u druge aplikacije, npr. Finder. Samo kopiranje, i to samo lokalnih datoteka.
- **Unutra**: iz Findera u panel.
- **Između panela**: ispuštanje otvara F5 dijalog s ciljem = mapa pod pokazivačem ili mapa panela. **Shift** pri ispuštanju daje F6 (premještanje). Dijalog ima i gumb *Move instead* / *Copy instead*, ako Shift promakne.

## Napomene

- **Trash (F8)**: fxfm premješta u Trash preko `NSFileManager`, a ne preko Findera. Brže je i ne traži dozvolu za Automation, ali Finderov "Put Back" ne radi za stavke koje je obrisao fxfm.
- **PDF u F3 vieweru**: PDF se prikazuje u iframeu preko asset protokola, a uz njega je uvijek vidljiv gumb "Open in Quick Look". Je li WKWebView stvarno iscrtava PDF u iframeu još nije ručno provjereno; ako ne, Quick Look je zamjena.
- **Full Disk Access**: za neke mape (Mail, Messages, Safari…) treba uključiti fxfm u System Settings → Privacy & Security → Full Disk Access. fxfm to objasni kad prvi put naiđe na zabranu.
- **Tipke koje macOS zauzima**: Ctrl+strelice (Mission Control, prostori) i Ctrl+Space (izvor unosa) macOS presreće prije fxfm-a. Configuration → Keyboard upozori ako je takva tipka dodijeljena i nudi otvaranje System Settings → Keyboard → Keyboard Shortcuts.
- Cmd se ne koristi za TC naredbe; iznimke su Cmd+Q i standardni Edit meni (Cmd+C/V/X/A u tekstualnim poljima).

## Zadane tipke

Tablica se generira iz registra naredbi (`COMMANDS` u `crates/core/src/keymap.rs`); test `readme_key_table_matches_commands` pada ako zastari i ispiše novu. Tipke se mijenjaju u Configuration → Keyboard ("Reset to TC defaults" vraća ove).

<!-- keys:start -->
| Menu | Command | Keys |
|---|---|---|
| Files | View | `F3` |
| Files | Edit | `F4` |
| Files | Edit new file | `Shift+F4` |
| Files | Copy | `F5` |
| Files | Move | `F6` |
| Files | Rename | `F2`, `Shift+F6` |
| Files | New folder | `F7` |
| Files | Delete | `F8`, `Delete` |
| Files | Delete permanently | `Shift+F8`, `Shift+Delete` |
| Files | Exit | `Alt+F4` |
| Mark | Mark/unmark | `Space`, `Insert` |
| Mark | Select group | `NumAdd`, `Ctrl+=` |
| Mark | Unselect group | `NumSubtract`, `Ctrl+-` |
| Mark | Invert selection | `NumMultiply`, `Ctrl+8` |
| Mark | Select all | `Ctrl+A` |
| Commands | Switch panel | `Tab` |
| Commands | Directory hotlist | `Ctrl+D` |
| Commands | Parent directory | `Backspace`, `Ctrl+PageUp` |
| Commands | Root directory | `Ctrl+\` |
| Commands | Swap panels | `Ctrl+U` |
| Commands | Target = Source | — |
| Commands | Search (later version) | `Alt+F7` |
| Files | Multi-rename (later version) | `Ctrl+M` |
| Files | Change attributes… | — |
| Net | Connect to server | `Ctrl+F` |
| Net | Disconnect | `Ctrl+Shift+F` |
| Show | Left drive | `Alt+F1` |
| Show | Right drive | `Alt+F2` |
| Show | Refresh | `Ctrl+R` |
| Show | Show hidden files | `F1`, `Ctrl+H` |
| Commands | Command line | `Ctrl+Enter` |
| Configuration | Keyboard… | — |
| Configuration | Options… | — |
<!-- keys:end -->

Fiksne tipke u panelu (ne remapiraju se): Up/Down, PageUp/PageDown, Home/End, **Left = prvi vidljivi red u prozoru**, **Right = zadnji vidljivi red u prozoru**; ako je kursor već tamo, sljedeći pritisak pomiče za cijeli prozor (prvi red prethodnog, odnosno zadnji red sljedećeg prozora), Enter.
