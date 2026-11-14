# fxfm roadmap

## Sljedeće
- **Pretraga (Alt+F7)**: fxfm ima **vlastiti indeks** datoteka, ne Spotlight (`mdfind`). Spotlight preskače skrivene datoteke i dotfiles, mrežne diskove i sadržaj arhiva. Prije plana dogovoriti: koje mape se indeksiraju, gdje se indeks sprema (u portable načinu putuje s `fxfm-data/`, pa može narasti), osvježavanje u pozadini (FSEvents).
- **Multi-rename (Ctrl+M)**.

## Serveri (nakon plana 3)
- Spajanje (15 s) se ne može prekinuti (nema Cancel).
- F4 upload na server čija je veza u međuvremenu izgubljena: nakon Reconnect upload se ne nudi ponovno, datoteku treba još jednom spremiti u editoru.
- Keychain kod se testira samo ručno (testovi ne diraju pravi Keychain): treba ručna provjera na pravom Macu.
- Povlačenje stavki iz arhiva i sa servera u druge aplikacije (vidi Drag & drop).
- Dok je panel u arhivi sa servera, traka diskova označava privremeni volumen.
- Preuzimanje arhive sa servera nema napredak ni Cancel.
- Unutar arhive sa servera zadani cilj F5/F6 i "add current folder" u hotlisti koriste privremenu putanju.

## Zvuk (F3)
- Uz *System default* promjena zadanog izlaznog uređaja u macOS-u (slušalice, AirPods) se ne primijeti dok player nije bio neaktivan. Uređaj izabran u Options vrijedi odmah.
- Izabrani uređaj koji nije spojen: svira se na zadanom; kad se uređaj ponovno spoji, koristi se tek nakon što se izlaz zatvori (60 s bez sviranja) ili otkaže.
- Uređaji se prepoznaju po imenu: ako dva imaju isto ime, koristi se prvi (u Options se ime pokazuje jednom).
- Preostalo: G.726 i ostali WAV kodeci osim PCM/float/A-law/mu-law/MS i IMA ADPCM/GSM 6.10/MP3 javljaju se kao nepodržani; nema pauze ni glasnoće (svira na 100 %).

## Arhive (nakon plana 2)
- F4 upload natrag ne radi za arhivu koja se nalazi u mapi čije ime izgleda kao arhiva (npr. `backup.zip/real.tar`); kopija ostaje sačuvana. Rješenje: zapamtiti stvarnu putanju arhive pri otvaranju i nju uspoređivati pri uploadu.
- Pakiranje novih arhiva (Alt+F5), ugniježđene arhive, tar.bz2/tar.xz, sukobi po datoteci (a ne po stavci) pri kopiranju u arhivu.

## Drag & drop
- Povlačenje iz arhiva i sa servera u druge aplikacije (Finder…) još nije podržano: radi samo između panela. Rješenje: `NSFilePromiseProvider` (datoteka se raspakira/preuzme tek kad je aplikacija primi).
- Slika povlačenja je Finderova ikona (jedne datoteke ili skupa), bez broja datoteka.
- Povlačenje van uvijek nudi samo kopiranje (`drag` crate zna samo jednu operaciju), pa Finder i na istom disku kopira, ne premješta.

## Gotovo
- **Plan 3 – serveri** (2026-11-14): SFTP/SCP/FTP/FTPS (Ctrl+F), ključevi po serveru (uvoz u `keys/`), master lozinka u portable načinu i Keychain u sistemskom, `known_hosts`, F3–F8/F2 i chmod na serveru, sigurni upload (`.part` pa zamjena), Connection lost + Reconnect, hotlista sa serverima, arhive na serveru (Enter preuzme praćenu kopiju, izmjene se nude za upload natrag).
- **Drag & drop** (2026-11-14): povlačenje redaka (označene stavke ako je pritisnuti redak označen, inače samo on; `..` nikad) iz panela u druge aplikacije kao pravo macOS povlačenje datoteka (`drag` crate, samo lokalne datoteke), iz Findera u panel i između panela. Ispuštanje otvara F5 dijalog (Shift = F6) s ciljem = mapa pod pokazivačem ili mapa panela; dijalog ima i gumb „Move instead“ / „Copy instead“. Ista mapa, sama povučena stavka ili mapa unutar povučene: ništa. Hotlist dijalog premješta retke mišem (HTML5 drag & drop ne radi uz Tauri drag-drop handler).
- **F3 preslušavanje zvuka** (2026-11-13): F3 na audio datoteci svira odmah u glavnom prozoru, fokus ostaje u panelu; traka iznad F-tipki (ime, `m:ss / m:ss`, kodek, klik na traku = skok); F3 na istoj datoteci ili Esc zaustavlja. Dekodiranje u Rustu (`symphonia` + `symphonia-opus`, vlastiti GSM 6.10 i `.au`): wav (PCM, float, A-law, mu-law, MS/IMA ADPCM, GSM WAV49, MP3-u-WAV), mp3, flac, ogg/vorbis, opus, m4a/aac, aiff, au, gsm. Radi i u arhivama i na serverima (privremena kopija se briše kad sviranje završi).
