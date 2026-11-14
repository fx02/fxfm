# fxfm (fx file manager) – dizajn v1

Datum: 2026-11-06
Autor zahtjeva: Zvonimir Buzanić
Projekt: `~/Documents/PVT/fxfm` (sestrinski projekt fxmp music playeru)

## 1. Namjera i zašto

Korisnik je godinama radio u Total Commanderu na Windowsima i prešao na Mac. Prelazak "nije bio lak". Na Macu nema file managera koji izgleda i ponaša se kao TC. fxfm je dvopanelni file manager za macOS koji **izgleda kao TC i ponaša se kao TC** (iste tipke, isti dijalozi, isti tok rada) te dodaje stvari koje korisnik svakodnevno treba:

- brzo kopiranje datoteka (php, shell skripte, zip/tar) na Linux servere u lokalnoj mreži, preko VPN-a ili na internetu, s vlastitim SSH ključevima po serveru
- otvaranje i uređivanje sadržaja arhiva kao da su mape

Uspjeh: korisnik otvori fxfm umjesto TC-a/Findera i radi "na slijepo" s mišićnom memorijom iz TC-a.

## 2. Ciljevi v1

1. Glavni prozor u TC izgledu (vidi §4), dva panela, Tab prebacuje fokus između njih.
2. Pregled diskova/particija: traka diskova i padajući izbornik diska.
3. Osnovne TC operacije i prečaci (vidi §5), sve remapabilno.
4. Ctrl+D hotlist lokacija (Change directory menu), kao u TC-u.
5. Ctrl+F lista servera i spajanje: SFTP, SCP, FTP, FTPS, s ključevima po serveru.
6. Arhive kao mape: zip, tar, tgz (tar.gz), gz, 7z. Čitanje i pisanje (kopiranje unutra/van, brisanje, rename, uređivanje datoteke unutar arhive).
7. F3 ugrađeni preglednik, F4 vanjski editor po izboru korisnika.
8. Portable ili sistemski način spremanja postavki, po izboru korisnika.

## 3. Ne-ciljevi v1 (za kasnije verzije)

Tabovi (Ctrl+T), usporedba direktorija, sinkronizacija direktorija, multi-rename, pretraga (Alt+F7), pluginovi, thumbnail prikaz, rar.

Korisnik planira postupno dodavati većinu TC funkcionalnosti. Prvi na redu nakon v1 su **pretraga (Alt+F7)** i **multi-rename (Ctrl+M)**, jer ih svakodnevno koristi. Arhitektura v1 zato mora omogućiti lako dodavanje novih naredbi: svaka naredba registrira se u središnji registar naredbi (ime, radnja, zadana tipka) koji pokreće meni, F-traku i remapiranje tipki. Alt+F7 i Ctrl+M se u v1 rezerviraju u registru.

### Anti-ciljevi (ako se ovo dogodi, v1 je promašaj iako "radi")

- Izgleda kao Finder ili generička Mac aplikacija, a ne kao TC.
- Tipke se ne ponašaju kao u TC-u, ili F-tipke ne rade direktno (korisnik ima uključeno "Use F1, F2… as standard function keys", F-tipke dolaze bez Fn).
- Prečaci rade na Cmd umjesto na Ctrl (TC koristi Ctrl i to se zadržava).
- Postavke, ključevi i lozinke su prisilno zaključani na sustav (Keychain/Library) bez mogućnosti portable načina.
- Uređivanje u arhivi ili na serveru tiho izgubi promjene.

## 4. Izgled (odobren mockup)

Reference u `tc-screenshots/` pored ovog dokumenta:
- `fxfm-mockup-approved.html`: odobreni mockup fxfm-a

Dijalozi (Ctrl+D, Ctrl+F) slijede raspored istoimenih dijaloga Total Commandera; snimke zaslona nisu dio repozitorija.

Odozgo prema dolje, za svaki panel:

- **Traka diskova**: gumbi s ikonom i imenom za `/` (Macintosh HD), `~` (home), svaki montirani volumen iz `/Volumes` (USB, DMG, SMB/AFP share) te svaka **aktivna** veza na server (☁️ ime). Aktivni disk je istaknut. Ažurira se automatski kad se volumen montira ili odmontira.
- **Red s putanjom**: combo za izbor diska, `[ime volumena]  X k of Y k free` (format brojeva kao TC, točka kao separator tisućica), desno `/` (korijen) i `..` (nadmapa) kao klikabilni gumbi. Za remote se umjesto slobodnog prostora prikazuje `[sftp://user@host]`.
- **Trenutna putanja** s filterom (`~/Documents/PVT/*.*`). Panel u fokusu ima istaknutu traku.
- **Lista**: crna pozadina, veliki font (približno kao na TC snimci), ikona i ime. Mape u `[zagradama]`, `[..]` prvi. Kursor je sivi redak. Sortiranje: mape pa datoteke, abecedno. Klik na zaglavlje kolone nije nužan u v1 jer je prikaz "brief/full" kao na snimci (samo ime).
- **Status linija** ispod panela: ime pod kursorom, `<DIR>` ili veličina, datum `dd.MM.yyyy HH:mm`, Unix prava (`drwxr-xr-x`).
- Ispod oba panela opcionalni **naredbeni redak** (`putanja>`), koji naredbu izvršava u Terminal.app u trenutnoj mapi. Zatim **traka F-tipki** (F3 View, F4 Edit, F5 Copy, F6 Move, F7 NewFolder, F8 Delete, Alt+F4 Exit). Klik na gumb radi isto što i tipka.

Boje (zadane, sve podesive u Configuration → Colors):

| tip | boja |
|---|---|
| mape, obične datoteke | bijela |
| skrivene (dotfiles, .DS_Store) | siva |
| arhive (zip, tar, tgz, gz, 7z) | narančasto-crvena (#ff6a4d) |
| video/audio | ljubičasto-plava |
| označene (Insert/Space) | crvena |

Glavni meni (Files, Mark, Commands, Net, Show, Configuration, Start, Help) nalazi se u **macOS gornjoj traci**, ne u prozoru. Naslov prozora: `korisnik – fxfm verzija`.

**Početna mapa:** kod **prvog** pokretanja oba panela kreću iz korisnikovog home direktorija (`~`, gdje su Desktop, Documents, Downloads itd.). Nakon toga fxfm **pamti zadnje stanje i vraća ga pri svakom pokretanju**: putanje oba panela, aktivni panel i poziciju kursora.

Pravila za vraćanje:
- Ako zapamćena putanja više ne postoji (odmontiran USB, obrisana mapa), panel ide na najbližu postojeću nadmapu, a ako ni nje nema, na `~`.
- Ako je panel bio na serveru ili u arhivi, panel se otvara na lokalnoj mapi iz koje se išlo (za server je to `~`). Moja odluka: fxfm se ne spaja automatski na server pri pokretanju.

Uvijek se pamte veličina i pozicija prozora.

## 5. Tipke i operacije

Sve tipke koriste fizički **Ctrl** (ne Cmd). Option na Macu služi kao Alt.

**Tipkovnica korisnika je Mac**: ugrađena MacBook tipkovnica ili Logitech MX Keys for Mac. Nema tipku Insert; numpad i Home/End/PgUp/PgDn postoje samo ponekad (na MacBooku su to Fn+strelice). Zadana mapa zato za svaku TC radnju koja ovisi o Insert/Num tipkama ima i **Mac alternativu**:
- Space označava (Insert ostaje vezan ako ga tipkovnica ima)
- Ctrl+= / Ctrl+− rade isto što Num + / Num −
- Ctrl+8 radi isto što Num *

Num tipke ostaju dodatno vezane za tipkovnice s numpadom.

**Konflikti sa sustavom**: macOS sam presreće neke Ctrl kombinacije (Ctrl+↑/↓ i Ctrl+←/→ za Mission Control, Ctrl+Space za promjenu jezika, eventualno Ctrl/Option+F-tipke). fxfm ih **ne preuzima nasilno** (nema CGEventTapa ni Accessibility dozvole). Kod prvog pokretanja, i u Configuration → Keyboard, prikazuje koje su zadane fxfm tipke zauzete u sustavu te nudi otvaranje System Settings → Keyboard Shortcuts ili remapiranje u fxfm-u. Za promjenu redoslijeda u hotlisti postoje i gumbi ↑/↓ te drag & drop.

| tipka | radnja |
|---|---|
| Tab | prebacivanje fokusa na drugi panel |
| ↑ ↓ PgUp PgDn Home End | kretanje kursora |
| Enter | uđi u mapu ili arhivu; za datoteku je otvori zadanom macOS aplikacijom |
| Backspace | nadmapa |
| Ctrl+\\ | korijen |
| Ctrl+PgUp | nadmapa (TC alternativa) |
| Insert / Space | označi ili odznači (Space na mapi ujedno računa veličinu) |
| Num + / Num − | označi ili odznači po masci (dijalog s `*.php` itd.) |
| Num * | invertiraj oznake |
| Ctrl+A | označi sve |
| F2 | rename na mjestu (korisnikov zahtjev) |
| Shift+F6 | rename na mjestu (TC default, ostaje i ovo) |
| F3 | ugrađeni preglednik |
| F4 | uredi u vanjskom editoru |
| Shift+F4 | nova datoteka i otvori u editoru |
| F5 | kopiraj u drugi panel (dijalog s odredištem, kao TC) |
| F6 | premjesti u drugi panel |
| F7 | nova mapa |
| F8 / Delete | obriši (potvrda; lokalno ide u Trash, opcija za trajno brisanje sa Shift+F8) |
| Alt+F1 / Alt+F2 | izbor diska za lijevi/desni panel |
| Ctrl+D | hotlist (§6) |
| Ctrl+F | spajanje na server (§7) |
| Ctrl+Shift+F | prekid veze |
| Ctrl+R | osvježi panel |
| Ctrl+U | zamijeni panele |
| Alt+F4 | izlaz (uz Cmd+Q) |

**Operacije F5/F6/F8** rade jednako za lokalno ↔ lokalno, lokalno ↔ server, server ↔ server (preko lokalnog privremenog spremnika), lokalno ↔ arhiva i arhiva ↔ server. Ako postoje označene datoteke, operacija radi nad njima; inače radi nad datotekom pod kursorom. Dugačke operacije prikazuju TC-ov progress dijalog (trenutna datoteka, ukupni napredak, Cancel) i ne blokiraju UI. Kod konflikta pita kao TC: Overwrite / Overwrite all / Skip / Skip all / Rename / Overwrite older.

**Remapiranje**: Configuration → Keyboard prikazuje popis svih naredbi s trenutnom kombinacijom, a korisnik pritisne novu kombinaciju. Postoje upozorenje na konflikt i gumb "Reset na TC zadane". Svaka tipka (uključujući F1–F12, Tab, Insert, Num tipke) može se dodijeliti bilo kojoj naredbi.

**F3 preglednik**: ugrađen prozor. Tekst s detekcijom kodiranja (UTF-8, Windows-1250 za hrvatske znakove), slike, PDF, a za ostalo hex. Esc ili F3 zatvara. Radi i za datoteke na serveru i u arhivi (prvo ih preuzme u privremenu mapu).

**F4 editor**: Configuration → Editor ima putanju do aplikacije ili naredbu (npr. Visual Studio Code, Sublime, ili `nano` u Terminalu). Za lokalnu datoteku otvara direktno. Za datoteku sa servera ili iz arhive:
1. preuzme je u privremenu mapu i otvori je u editoru;
2. prati tu kopiju (editori poput VS Codea ne blokiraju dok se ne zatvore, pa se ne čeka zatvaranje procesa);
3. kod svake promjene spremljene datoteke pita kao TC: "Datoteka je promijenjena. Učitati natrag na server/u arhivu?" (Da / Ne);
4. privremene kopije čiste se pri zatvaranju veze ili izlasku.

## 6. Ctrl+D hotlist ("Change directory menu")

- Ctrl+D otvara popup meni na mjestu kursora: prvo "Add current dir" i "Configure…", pa korisnikove stavke i podmeniji. Stavke imaju TC-ovo `&` za tipku-akcelerator.
- Configure otvara dijalog 1:1 kao TC (druga snimka): lista stavki, gumbi Add Item, Add submenu, Delete Item, Change Title, OK, Cancel, Help, polja **Command** (`cd /putanja`) s gumbom za izbor mape, **Target path** (ako je zadano, drugi panel ide tamo) te "Hide menu items starting with this character" (dvoklik skriva ili prikazuje stavku).
- Promjena redoslijeda drag & dropom ili Ctrl+↑/↓.
- Command može biti i remote: `cd sftp://web-1/var/www/html`, gdje je `web-1` ime iz liste servera. Odabir se spaja (ako već nije spojen) i otvara tu mapu.

## 7. Serveri (Ctrl+F)

**Dijalog "Connect to server"** je 1:1 s TC-om (treća snimka): stablo s mapama i serverima, gumbi Connect, New connection…, New URL…, Duplicate entry…, New folder…, Edit…, Delete, Encrypt…, Cancel, Help. Dvoklik ili Enter spaja.

**Podaci po serveru**:
- naziv
- protokol: SFTP, SCP, FTP, FTPS (explicit TLS); SCP koristi SSH vezu, a lista mape preko SFTP-a ili `ls` ako server nema SFTP
- host, port
- korisnik
- autentikacija: lozinka, privatni ključ ili ključ + passphrase
  - ključ se može **odabrati s lokacije** na disku (putanja se pamti) ili **importati** (kopira se u `keys/` u fxfm podatkovnoj mapi i putuje s njom)
  - podržani OpenSSH formati (RSA, ECDSA, Ed25519) i PEM
- početni direktorij lokalno i na serveru
- opcija "Dopusti zastarjele algoritme (ssh-rsa/SHA1)" za stare servere
- FTP: pasivni ili aktivni način, kodiranje imena
- opcija "Spremi lozinku"

**Lozinka za ključ/server**: ako nije spremljena, pita kod spajanja.

**Host key**: kod prvog spajanja prikazuje se fingerprint i pitanje za prihvaćanje. Prihvaćeni ključevi spremaju se u fxfm `known_hosts` u podatkovnoj mapi. Kod promjene ključa slijedi glasno upozorenje.

Nakon spajanja server se pojavljuje kao "disk" u traci diskova i panel ga pregledava kao mapu. Ostaju dostupne sve operacije: F3, F4, F5, F6, F7, F8, F2, a za SFTP i promjena prava (Files → Change attributes, chmod). Više istovremenih veza je dozvoljeno. Prekid veze je Ctrl+Shift+F. Ako veza pukne, panel to jasno prikazuje i nudi ponovno spajanje.

**New URL…** prima `sftp://user@host:port/path` ili `ftp://…` za brzo spajanje bez spremanja.

## 8. Arhive

- Enter na `.zip`, `.tar`, `.tgz`/`.tar.gz`, `.gz`, `.7z` ulazi u arhivu kao u mapu. `[..]` u korijenu arhive vraća u mapu koja sadrži arhivu. Putanja se prikazuje kao `/put/do/arhiva.zip/unutarnja/mapa`.
- Unutar arhive rade F3, F4 (s "upload back" tokom kao za server), F5 (van i unutra), F6, F7, F8 i F2.
- `.gz` (bez tar-a) sadrži jednu datoteku. Prikazuje se kao arhiva s jednom datotekom.
- Pisanje u arhivu radi preko nove privremene arhive koja **atomski zamjenjuje** original tek nakon uspjeha. Original se nikad ne smije pokvariti ako operacija ne uspije.
- Arhive na serveru: Enter je preuzme u privremenu mapu i otvori. Promjene se nude za upload natrag kao kod F4.
- Lozinkom zaštićeni zip: pita lozinku za čitanje. 7z s lozinkom i pisanje u kriptirane arhive nisu u v1 (libarchive ne čita kriptirani 7z; ako zatreba, to je zaseban backend kasnije).
- Pakiranje novih arhiva (Alt+F5 u TC-u) nije u v1, osim ako je trivijalno uz istu biblioteku.

## 9. Postavke i portable način

Configuration → Options → **Način spremanja**:

- **Portable**: sve u `fxfm-data/` pored `fxfm.app` (config, hotlist, serveri, keymap, boje, known_hosts, `keys/`). Lozinke i passphraseovi kriptiraju se **master lozinkom** (gumb Encrypt… u Ctrl+F dijalogu, kao TC). Master lozinka se traži jednom po pokretanju, kad zatreba. Bez master lozinke lozinke se ne spremaju.
- **Sistemski**: postavke u `~/Library/Application Support/fxfm/`, lozinke u macOS Keychainu.

Pravila:
- Ako postoji `fxfm-data/` pored aplikacije i zapisiva je, koristi se portable. Inače se koristi sistemski način.
- Prebacivanje načina migrira sve podatke (uključujući lozinke) i to jasno potvrđuje.
- Formati su čitljivi tekst (JSON ili INI), tako da ih korisnik može ručno uređivati ili verzionirati.

## 10. Ograničenja

- macOS 15 (Sequoia) ili noviji. Razvojni Mac ima macOS 15.7, Rust 1.99.1, tauri-cli 2.12, Node 26.
- Aplikacija mora biti samostalna: sve biblioteke (SSH, FTP, arhive) **ugrađene u .app**. Bez Homebrewa ili drugih instalacija na ciljnom Macu, jer portable mora raditi kopiranjem mape.
- Lokalni pristup cijelom disku traži Full Disk Access za neke mape. Pri prvom pokretanju aplikacija to objašnjava ako naiđe na zabranu.
- Distribucija: osobna upotreba, build lokalno. Potpisivanje/notarizacija nisu potrebni u v1 (ad-hoc potpis).
- **Nošenje na druge Macove** (USB, mrežna kopija, zip) je očekivano:
  - Kopirana aplikacija dobije quarantine atribut, pa je Gatekeeper može blokirati, a App Translocation je pokreće s nasumične read-only putanje. Tada se `fxfm-data/` pored .app ne može pronaći.
  - fxfm mora prepoznati translociranu putanju (`SecTranslocateIsTranslocatedURL` ili putanja pod `/private/var/folders/.../AppTranslocation/`). Tada prikazuje jasnu poruku s rješenjem: naredba `xattr -dr com.apple.quarantine <putanja>/fxfm.app` koju može kopirati ili izvršiti jednim klikom, nakon čega se aplikacija ponovno pokrene.
  - fxfm ne smije tiho prijeći u sistemski način ni krenuti s praznim postavkama.
  - Uz projekt ide kratki README "kako prenijeti na drugi Mac".
- Git commitovi za ovaj projekt samo kao `zbuzanic@gmail.com` (isto pravilo kao za fxmp).

## 10a. Platforma i Linux

fxfm je **macOS-first** projekt na **Rust + Tauri 2**, po uzoru na korisnikov fxmp (`~/Documents/PVT/fxmp`): Cargo workspace s `crates/core` + `src-tauri`, UI u čistom JS/HTML/CSS bez bundlera (`ui/`), Rust verzija pinana u `rust-toolchain.toml`. Odluku je donio korisnik ("ako je Rust bolji, vozi Rust"), jer tako i Linux build dolazi gotovo besplatno.

- **fxfm-core** (`crates/core`): sva logika, i to VFS (lokalno, arhiva, SFTP/SCP/FTP), operacije s progressom i konfliktima, registar naredbi i keymap, konfiguracija, kriptiranje lozinki, hotlista i lista servera. Bez ovisnosti o Tauriju. Mora se buildati na macOS-u i Linuxu.
- **src-tauri**: tanak sloj koji naredbe izlaže UI-ju, plus macOS specifičnosti (meni u gornjoj traci, Keychain preko `keyring`, Trash, Terminal.app).
- **ui/**: TC izgled, crtanje panela, tipkovnica.
- Linux build nije cilj v1, ali kod ne smije imati macOS-only pretpostavke izvan `cfg(target_os = "macos")` blokova.

## 11. Odluke koje je donio Claude (korisnik ih nije eksplicitno birao)

- Meni u macOS gornjoj traci.
- Unix prava umjesto Windows atributa u status liniji.
- Zadane boje iz §4.
- F8 lokalno briše u Trash (Shift+F8 trajno).
- Server ↔ server kopiranje ide preko lokalnog privremenog spremnika.
- Remote stavke u hotlisti (`cd sftp://ime/putanja`).
- Mac alternative za Insert/Num tipke (Space, Ctrl+=, Ctrl+−, Ctrl+8) i bez preuzimanja sistemskih prečaca (§5).
- 7z s lozinkom nije u v1.
- Podjela na fxfm-core (prenosiv na Linux) i Tauri UI (§10a).
- **Universal build (arm64 + x86_64)**, jer korisnik možda nosi fxfm na druge Macove i ne zna se jesu li svi Apple Silicon. Sve C biblioteke grade se kao universal.
- Nema importa iz Total Commandera (korisnik eksplicitno ne treba).
- Server ili arhiva se ne vraćaju automatski pri pokretanju (panel ide na lokalnu mapu), a nepostojeća putanja pada na najbližu nadmapu.

## 12. Prepušteno builderu

- Izbor crateova. Prednost imaju čisti Rust crateovi ili zreli bindingi koji se statički linkaju u bundle:
  - SFTP/SCP: npr. `ssh2` s vendored OpenSSL-om ili `russh`
  - FTP/FTPS: npr. `suppaftp`
  - arhive: `zip`, `tar`, `flate2`, `sevenz-rust2`
- Arhitektura VFS sloja (zajednički trait za lokalno, arhivu i remote je očekivan).
- Format konfiguracijskih datoteka, kriptografija za master lozinku (standardni KDF + AEAD).
- Ikone datoteka.
- Organizacija koda i testova (po uzoru na fxmp).
