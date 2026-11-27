# Idee da terminali moderni/web, tiling e Git UX (ttyd, Tilix, fancy-git)

## 1. ttyd: come espone un terminale sul web e cosa significherebbe per Oxterm

### Takeaway
ttyd è un sottile ponte C che avvia un processo qualsiasi su una PTY e ne multiplexa i byte su WebSocket verso un frontend xterm.js nel browser, con auth/SSL/reverse-proxy/embedding tutti opzionali. Per Oxterm l'idea trasferibile NON è «rifare xterm.js in VTE», ma aggiungere un bridged headless PTY↔WebSocket più un client web, riutilizzando l'API di controllo già esistente.

### Cited Findings
- ttyd si descrive come «Share your terminal over the web»: costruito su libwebsockets + libuv, terminale basato su xterm.js con supporto CJK e IME, ZMODEM/lrzsz e trzsz per trasferimento file, output immagini Sixel, SSL via OpenSSL, esecuzione di un comando arbitrario con opzioni — [README](https://github.com/tsl0922/ttyd)
- Il manuale conferma l'architettura (libwebsockets + libuv, frontend xterm.js) e la porta predefinita 7681; il client è **read-only di default** e richiede `-W/--writable` per poter scrivere sulla TTY — [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1)
- Autenticazione e sicurezza: `-c/--credential username:password` (HTTP Basic), `-H/--auth-header` per delegare l'autenticazione a un reverse proxy, `-S/--ssl` con cert/chave/CA per SSL e verifica del certificato client (mTLS), `-O/--check-origin` per rifiutare WebSocket da origini diverse, `-i` per bind su socket UNIX con proprietario `-U`, `-u/-g` per drop dei privilegi — [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1)
- Embedding: `-I/--index` per un index.html personalizzato, `-b/--base-path` per servire sotto un sottopercorso del proxy (max 128 caratteri), configurazione Nginx di esempio con `Upgrade`/`Connection: upgrade` e `X-Forwarded-*` — [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1); [wiki Nginx reverse proxy](https://github.com/tsl0922/ttyd/wiki/Nginx-reverse-proxy)
- Client options `-t chiave=valore`: `rendererType=canvas` (default webgl), `disableLeaveAlert`, `disableResizeOverlay`, `disableReconnect`, `enableZmodem`, `enableTrzsz`, `enableSixel`, `closeOnDisconnect`, `titleFixed`, `fontSize`, `unicodeVersion`, `trzszDragInitTimeout` (default 3000 ms) e qualunque `ITerminalOptions` di xterm.js (`cursorStyle`, `lineHeight`, `theme` JSON) — [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1)
- Ciclo di vita client: `-m/--max-clients`, `-o/--once` (un client poi esce), `-q/--exit-no-conn`, `-P/--ping-interval` (default 5 s), `-s/--signal` inviato al comando alla fine, `-w/--cwd`, `-a/--url-arg` per argomenti nella URL — [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1)
- Più client possono condividere lo stesso processo tramite `ttyd tmux new -A -s ttyd vim`; ttyd può eseguire anche non-shell come `ttyd vim` o `ttyd login` — [wiki Example Usage](https://github.com/tsl0922/ttyd/wiki/Example-Usage)
- API di embedding JS: ttyd espone l'istanza xterm.js come `window.term` con un metodo `fit()`; da una pagina che include ttyd in iframe si accede via `iframe.contentWindow.term` e si può fare `term.paste("comando")` — [wiki Programming interface](https://github.com/tsl0922/ttyd/wiki/Programming-interface)
- Per l'uso "desktop-like" la wiki suggerisce `chrome --app=https://.../ttyd` così Ctrl+N/Ctrl+W arrivano al terminale invece che al browser — [wiki Example Usage](https://github.com/tsl0922/ttyd/wiki/Example-Usage)
- Il supporto browser è delegato a xterm.js («Modern browsers»); non esiste una UI mobile dedicata documentata — [README](https://github.com/tsl0922/ttyd), [wiki Example Usage](https://github.com/tsl0922/ttyd/wiki/Example-Usage)

### Inferences
- Il modello chiave è **separare proprietario della PTY e renderer**: il server possiede il processo, il browser è solo un terminale virtuale. Oxterm potrebbe aggiungere un `oxterm bridge --port ...` che avvia PTY headless e parla WebSocket, lasciando VTE fuori dal percorso remoto.
- Oxterm ha già la superficie di comandi interni che un bridge dovrebbe inoltrare: `oxterm new|split|send|capture|wait` e un dispatcher di azioni MCP (`terminal_split`, `notify`, ecc.) — [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs). Un bridge potrebbe mappare messaggi WebSocket su queste stesse azioni invece di duplicare logica.
- Il default read-only di ttyd è un'idea direttamente adottabile: un modo «condividi vista» in cui il browser riceve output ma non può scrivere, con opt-in esplicito per la scrittura.
- La triade `--auth-header` + `--base-path` + `--check-origin` è la lezione operativa: non esporre mai WebSocket in chiaro su 0.0.0.0 senza reverse proxy con TLS, controllo origine e limiti di client.
- Le client options mostrano come rendere configurabile il frontend senza ricompilare: stesso schema potrebbe essere applicato alle opzioni VTE di Oxterm tramite `--client-option`-like o impostazioni in `spec.rs` — [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1); [AGENTS.md](https://github.com/buzzqw/oxterm/blob/master/AGENTS.md)
- L'embedding in iframe con `window.term` suggerisce per Oxterm una modalità «pannello web»: invece di reimplementare il rendering, incorporare una pagina ttyd/xterm.js in un GtkBox (richiederebbe però un WebKitGTK non presente oggi) — [wiki Programming interface](https://github.com/tsl0922/ttyd/wiki/Programming-interface); [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml)

### Gaps
- Non è stata verificata l'implementazione interna in `src/` di ttyd (framing WebSocket, reconnect, flow control, backpressure): le fonti consultate sono README, man e wiki.
- Nessuna evidenza di supporto a layout multi-pannello lato browser: ttyd mostra un terminale per pagina e delega split a tmux — [wiki Example Usage](https://github.com/tsl0922/ttyd/wiki/Example-Usage)
- "Mobile use" è solo deduzione dal supporto browser di xterm.js; non ho trovato documentazione su gesture, barra tasti virtuale o layout responsive dedicati.
- Non ho verificato se esistano binding Rust mantenuti per libwebsockets; la scelta della crate andrebbe valutata a parte.

## 2. Tilix: tiling, raggruppamento terminali, notifiche e UX GTK

### Takeaway
Tilix è l'analogo più vicino a Oxterm: stesso stack GTK3+VTE, con un modello ad albero di split, drag&drop e salvataggio dei gruppi che Oxterm può adottare quasi letteralmente; le sue notifiche/badge però storicamente richiedono VTE patchato, mentre Oxterm ha già OSC 133 + notifiche desktop che risolvono il caso d'uso con VTE stock.

### Cited Findings
- Tilix è un emulatore terminale tiling per Linux basato su GTK+3 e widget VTE, scritto in D con gtkd, licenza MPL-2.0; il README avvisa che il progetto cerca maintainer e non implementerà nuove funzionalità — [README](https://github.com/gnunn1/tilix)
- Feature list: split orizzontali/verticali in qualsiasi disposizione; riarrangiamento drag&drop dentro e tra finestre; distacco in nuova finestra sempre via drag&drop; tab o sidebar per elencare le sessioni; **input sincronizzato** tra terminali; salvataggio/caricamento dei gruppi di terminali su disco; titoli personalizzati; schemi colore come file; sfondo trasparente e immagini; modalità Quake; hyperlink personalizzati; cambio automatico profilo per hostname e directory; notifiche al completamento di processi fuori vista (**richiede patch Fedora per VTE**); trigger e badge sperimentali (**richiedono VTE patchato**) — [README](https://github.com/gnunn1/tilix)
- Requisiti: GTK 3.18+, GTK VTE 0.46+, dconf/GSettings; usa CSD/HeaderBar e segue le GNOME HIG, ma CSD può essere disattivato — [README](https://github.com/gnunn1/tilix)
- Cambio profilo automatico: condizioni su username, hostname, directory corrente; formato `username@hostname:directory` con delimiter `@` o `:`; per SSH remoto serve il sourcing di `/usr/share/tilix/scripts/tilix_int.sh` nel `.bashrc` remoto oppure un alias `ssh='printf "\033]7;file://%s/\007" "remote-server.example.com"; ssh'` che emette OSC 7; quando il cambio automatico è attivo il menu profili manuale viene disabilitato — [wiki Automatic (Triggered) Profile Switching](https://github.com/gnunn1/tilix/wiki/Automatic-(Triggered)-Profile-Switching)
- Badge: testo mostrato nello sfondo del terminale, configurato per profilo, supporta le stesse variabili dei titoli (`${title}`), colore configurabile anche nei file tema; **richiede patch specifica Tilix per VTE** (su Arch il pacchetto `vte3-tilix`) — [wiki Badges](https://github.com/gnunn1/tilix/wiki/Badges)
- Quake mode: `--quake` crea o toggla una finestra a comparsa dall'alto; Tilix **non registra hotkey globale** perché Wayland non li supporta; l'utente deve bindare `tilix --quake` nel DE; su Wayland serve `GDK_BACKEND=x11` e la scelta del monitor secondario è disabilitata — [wiki Quake Mode](https://github.com/gnunn1/tilix/wiki/Quake-Mode)
- Azioni da riga di comando: `-a/--action` con `session-add-right`, `session-add-down`, `app-new-window`, `app-new-session`, `-e` per il comando da eseguire; l'azione è sempre relativa al terminale corrente e i parametri passano al nuovo terminale — [wiki Command-Line Actions](https://github.com/gnunn1/tilix/wiki/Command-Line-Actions)

### Inferences
- L'albero di tiling si mappa direttamente su `GtkPaned` annidati; Oxterm usa già `gtk::Paned` per i pannelli laterali — [connections_ui.rs](https://github.com/buzzqw/oxterm/blob/master/src/connections_ui.rs). Aggiungere DnD per riarrangiare/staccare è GTK3 puro, senza patch VTE.
- «Salva/carica gruppi» si mappa su serde di un albero di layout: Oxterm ha già `serde`/`serde_json` e il ripristino sessione — [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml).
- Input sincronizzato = broadcast dei tasti a più VTE; fattibile, ma andrebbe disabilitato per comandi sensibili (password/token) per evitare leak.
- Cambio profilo per host/dir: Oxterm conosce già l'URI di connessione (SSH/RDP/VNC) e ha shell integration OSC 133 (`osc133_fifo`), quindi può colorare/badgeare i pannelli remoti senza richiedere script sul server — [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs), [remote.rs](https://github.com/buzzqw/oxterm/blob/master/src/remote.rs).
- Notifica a comando finito: Oxterm già invia notifiche desktop e gestisce OSC 133 — [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs). È il modo realistico di ottenere la feature Tilix con VTE stock, evitando le patch Fedora.
- Badge nello sfondo: non fattibile con VTE stock senza patch; alternativa praticabile è un overlay GTK (GtkOverlay/Revealer) sopra il widget VTE oppure un badge nel tab/workspace.
- Quake mode: fattibile come `oxterm --quake` che toggla una finestra senza decorazioni in alto; ma l'hotkey globale va delegato al DE su Wayland, esattamente come Tilix — [wiki Quake Mode](https://github.com/gnunn1/tilix/wiki/Quake-Mode). Oxterm ha già un binario di controllo scriptabile che può essere il target dell'hotkey — [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs)
- Le azioni CLI di Tilix sono l'analogo diretto dell'API `oxterm split/send/...`: arricchirla con `split-left`, `split-up`, `focus-next`, `detach` darebbe lo stesso livello di automazione — [wiki Command-Line Actions](https://github.com/gnunn1/tilix/wiki/Command-Line-Actions); [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs)

### Gaps
- Non ho ispezionato il sorgente Tilix (`source/`) per la struttura dati esatta dell'albero di layout e il formato di serializzazione dei gruppi.
- Non ho verificato se VTE 0.70 (usato da Oxterm) includa nativamente gli equivalenti delle patch Fedora per notifiche e dei badge Tilix: le wiki parlano esplicitamente di VTE 0.46 patchato — [README](https://github.com/gnunn1/tilix), [wiki Badges](https://github.com/gnunn1/tilix/wiki/Badges)
- La pagina Triggers non è stata letta nel dettaglio: il meccanismo di matching del testo terminale è noto solo dai riferimenti indiretti nel README e nelle wiki.

## 3. fancy-git: pattern Git-TUI/UX per i flussi Git di Oxterm

### Takeaway
fancy-git non è una TUI ma un modulo prompt bash + alias: la sua forza è il vocabolario di stato Git compatto e la configurabilità tema/feature. Oxterm può portare questi pattern nella status bar/tab e nella command palette, senza copiare il codice bash né la dipendenza dai font Nerd.

### Cited Findings
- fancy-git è uno strumento bash per Linux/macOS che cambia il prompt per mostrare informazioni Git nel repository e tenere aggiornato lo stato del branch corrente — [README](https://github.com/diogocavilha/fancy-git)
- Le informazioni di stato che il prompt sa mostrare, dedotte dalle variabili di override `FANCYGIT_ICON_*`: presenza repo Git, branch solo locale, branch locale/remoto, branch merged, stash, file untracked, file changed, file added, commit non pushati, virtualenv Python — [README](https://github.com/diogocavilha/fancy-git)
- Temi (`default`, `human`, `simple`), numerosi color scheme (dracula, batman, robin, superman, light, neon, cga), stili separatore (default, blocks, blocks-tiny, curve, fire, lego, paint) e «feature switcher» `--enable-/--disable-` per: percorso completo, user@machine, orologio, prompt a due righe, notifiche ricche, bold, hostname, symbol utente — [README](https://github.com/diogocavilha/fancy-git)
- Config Git suggerita (`color.ui true`, `color.diff.meta/old/new`, `color.status.added/changed/untracked`) con comandi `--suggested-global-git-config[-apply]`; il colore dello sfondo del branch dipende dallo stato del repo — [README](https://github.com/diogocavilha/fancy-git)
- Set di alias: `gs`, `ga`/`gap`/`gaa`, `gd`, `gcm`/`gcmm`, `gpl`/`gps`/`gplo`/`gpso`/`gplob`/`gpsob`, `gck`/`gckb`/`gckbt`/`gcb`/`ggdb`/`gdbr`/`grb`/`grba`, `gl`/`glp`, `gdof`, `gfi`, `gurl`, `guser`, `gst`/`gstl`/`gstm`/`gstw`/`gsta`/`gstd`, `gg` (add --all + commit + push), `gu`, `pve`; `--enable-git-clear` controlla se gli alias puliscono il terminale — [README](https://github.com/diogocavilha/fancy-git)
- Architettura: `prompt.sh` fa source di `fancygit-completion`, `aliases`, `version.sh`, `modules/settings-manager.sh`, `modules/git-manager.sh`, `modules/update-manager.sh`, `commands-handler.sh`, `theme-functions.sh`, poi carica `themes/default.sh|human.sh|simple.sh` in base a `~/.fancy-git/app_config` — [prompt.sh](https://raw.githubusercontent.com/diogocavilha/fancy-git/master/prompt.sh)
- Richiede font Nerd Font specifici installati nel terminale (`fancygit --fonts-install`) per renderizzare le icone; la tabulazione su `fancygit --separator<TAB>` mostra gli stili — [README](https://github.com/diogocavilha/fancy-git)

### Inferences
- Il vocabolario di stato è direttamente mappabile a una badge della status bar / del tab di workspace di Oxterm: branch + conteggi (added/changed/untracked/unpushed/stash), calcolati in un helper display-free sotto `src/headless/` con `git status --porcelain=v2 -b` (il progetto già spawna processi) — [README](https://github.com/diogocavilha/fancy-git); [AGENTS.md](https://github.com/buzzqw/oxterm/blob/master/AGENTS.md)
- I feature switcher suggeriscono di implementare le opzioni in `src/headless/spec.rs` (unica fonte per CLI, impostazioni, help, man, completions, docs) invece che in un file di config shell dedicato — [AGENTS.md](https://github.com/buzzqw/oxterm/blob/master/AGENTS.md)
- Gli alias diventano voci di command palette con descrizione e completamento argomenti; `gg` è un'azione composta (add+commit+push) andrebbe resa esplicita/confermabile, non silenziosa — [README](https://github.com/diogocavilha/fancy-git)
- Non copiare la dipendenza Nerd Font: i glyph delle icone fancy-git richiedono font specifici, mentre Oxterm può usare icone GTK/glib o simboli Unicode già disponibili nel font VTE — [README](https://github.com/diogocavilha/fancy-git); [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml)
- La semantica dei color scheme andrebbe riusata nei temi esistenti di Oxterm (`assets/themes.json`, `src/headless/themes.rs`) invece di duplicarla: per le collisioni di nome valgono già le palette di Oxterm — [AGENTS.md](https://github.com/buzzqw/oxterm/blob/master/AGENTS.md)
- `--enable-git-clear` si traduce in una preferenza «pulisci il pannello prima dell'output dei comandi Git» — [README](https://github.com/diogocavilha/fancy-git)

### Gaps
- Non ho letto `modules/git-manager.sh`: restano sconosciuti i comandi Git esatti, la strategia di caching e l'impatto su repository grandi.
- Non esiste una TUI full-screen: fancy-git è solo prompt + alias + completion; descriverlo come «Git TUI» sarebbe inaccurato — [README](https://github.com/diogocavilha/fancy-git)
- Non ho trovato dichiarazioni upstream sul supporto Windows, a parte una nota sugli screenshot su come solo un certo font funzioni in Windows Terminal — [README](https://github.com/diogocavilha/fancy-git)

## 4. Fattibilità in GTK3/VTE Rust: cosa portare e cosa no

### Takeaway
Fattibili con le dipendenze attuali: tiling DnD, layout salvati, profili per host/dir, notifiche via OSC 133, azioni CLI estese, indicatori Git in status bar. Non fattibili senza nuove dipendenze o patch: rendering browser-embedding nativo, badge dentro VTE e hotkey globali su Wayland. Un bridge WebSocket è fattibile come nuovo binario headless, ma richiede nuove crate e un frontend web.

### Cited Findings
- Le dipendenze attuali di Oxterm sono GTK3 0.18, gio/gdk/pango/cairo, `zoha-vte` 0.6 con feature `v0_70`, `zoha-vte-sys`, sourceview4 0.5 per l'editor integrato, serde/serde_json, rusqlite, regex, nix, libc, shell-words; **non ci sono** crate HTTP/WebSocket/TLS o WebKit — [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml)
- Convenzioni architetturali: logica senza display in `src/headless/` (verificata dal test `tests/headless.rs`), CLI/impostazioni/help/man/completions/`docs/settings.md` generati da `src/headless/spec.rs`, temi built-in da `assets/themes.json` via `scripts/update-themes.py`, palette proprie in `src/headless/themes.rs` con priorità — [AGENTS.md](https://github.com/buzzqw/oxterm/blob/master/AGENTS.md)
- Oxterm possiede già: API di controllo `new|split|send|capture|wait`, dispatcher azioni MCP, enumerazione `foreign_panes()`, OSC 133 FIFO, `send_desktop_notification()`, `split_terminal()` — [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs)
- I pannelli laterali usano `gtk::Paned` con posizione persistente — [connections_ui.rs](https://github.com/buzzqw/oxterm/blob/master/src/connections_ui.rs)
- I badge Tilix richiedono una patch VTE specifica; le notifiche Tilix richiedono le patch Fedora per VTE — [wiki Badges](https://github.com/gnunn1/tilix/wiki/Badges); [README](https://github.com/gnunn1/tilix)
- Wayland non supporta hotkey globali; Tilix delega il binding al DE e usa `GDK_BACKEND=x11` — [wiki Quake Mode](https://github.com/gnunn1/tilix/wiki/Quake-Mode)
- ttyd richiede lato server libwebsockets+libuv e lato browser xterm.js/WebGL2 — [README](https://github.com/tsl0922/ttyd)

### Inferences
- **Fattibile subito (zero nuove dipendenze):** drag&drop e distacco pani in GTK3; serializzazione dell'albero di split con serde; input sincronizzato; titoli personalizzati; cambio profilo/tema in base a URI host o cwd; notifiche «comando finito» via OSC 133 già presente; estensione dell'API di controllo con azioni di split/focus; status bar Git con conteggi; command palette con alias Git e completamento.
- **Fattibile con moderate dipendenze nuove:** un bridge WebSocket: aggiungere una crate server HTTP/WS (es. basata su tokio/tungstenite) in un binario separato che possiede le PTY e inoltra i messaggi alle azioni dell'API di controllo — [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml); [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs). Va tenuto in `src/headless/` per rispettare il confine display-free — [AGENTS.md](https://github.com/buzzqw/oxterm/blob/master/AGENTS.md)
- **Non fattibile/rischioso senza altro lavoro:** (a) badge disegnati dentro l'area VTE come Tilix, perché serve VTE patchato — [wiki Badges](https://github.com/gnunn1/tilix/wiki/Badges); (b) notifiche «a livello VTE» stile Tilix, ma coperte funzionalmente da OSC 133 in Oxterm — [control.rs](https://github.com/buzzqw/oxterm/blob/master/src/control.rs); (c) un terminale xterm.js embedato inside GTK3 servirebbe WebKitGTK, oggi assente — [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml); (d) hotkey globali Quake su Wayland, per limitazione del protocollo — [wiki Quake Mode](https://github.com/gnunn1/tilix/wiki/Quake-Mode)
- **Priorità suggerita:** 1) DnD/distacco e layout salvati (affinità massima con Oxterm); 2) badge host/branch in tab e notifiche OSC 133; 3) Git status in status bar + palette alias; 4) bridge web solo come progetto separato, con auth/reverse-proxy fin dal giorno uno, seguendo ttyd — [README](https://github.com/gnunn1/tilix); [README](https://github.com/diogocavilha/fancy-git); [man ttyd.1](https://raw.githubusercontent.com/tsl0922/ttyd/main/man/ttyd.1)

### Gaps
- Non ho verificato l'effettiva presenza di API OSC 7/OSC 777 native in `zoha-vte` 0.6 / VTE 0.70: la fattibilità del cambio profilo automatico è stata dedotta dall'OSC 133 già implementato e dal comportamento documentato da Tilix.
- Non ho valutato licenze di compatibilità (MPL-2.0 Tilix, MIT ttyd, GPL-2.0 fancy-git per il prompt, EUPL-1.2 Oxterm) oltre a notarle: eventuale riutilizzo di codice andrebbe verificato — [README](https://github.com/gnunn1/tilix); [README](https://github.com/tsl0922/ttyd); [README](https://github.com/diogocavilha/fancy-git); [Cargo.toml](https://github.com/buzzqw/oxterm/blob/master/Cargo.toml)
- Nessuna stima di effort o benchmark: le valutazioni di fattibilità sono qualitative e basate sulla struttura del codice, non su prototipi.
