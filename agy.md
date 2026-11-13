# Analisi Architetturale, Prestazionale e di Sicurezza di Oxterm

> Documento di analisi tecnica approfondita, identificazione dei colli di bottiglia, anti-pattern, vulnerabilità e piano di ottimizzazione per **Oxterm** (v1.1.32).

---

## Indice dei Contenuti

1. [Panoramica Esecutiva](#1-panoramica-esecutiva)
2. [Anti-Pattern Architetturali & Qualità del Codice](#2-anti-pattern-architetturali--qualità-del-codice)
   - 2.1 [I Monoliti "God Objects" (`window.rs`, `terminal.rs`, `settings_dialog.rs`)](#21-i-monoliti-god-objects-windowrs-terminalrs-settings_dialogrs)
   - 2.2 [Duplicazione Massiccia tra `MainWindow` e `DetachedWindow`](#22-duplicazione-massiccia-tra-mainwindow-e-detachedwindow)
   - 2.3 [Violazione SRP in `src/headless/connections.rs` e Binary Bloat in `spec.rs`](#23-violazione-srp-in-srcheadlessconnectionsrs-e-binary-bloat-in-specrs)
   - 2.4 [Anti-Pattern Modale `dialog.run()` (35+ istanze)](#24-anti-pattern-modale-dialogrun-35-istanze)
   - 2.5 [Campi Fantasma (Dead Code) in `TerminalBox`](#25-campi-fantasma-dead-code-in-terminalbox)
3. [Colli di Bottiglia Prestazionali & Blocco del Thread GTK](#3-colli-di-bottiglia-prestazionali--blocco-del-thread-gtk)
   - 3.1 [`sync_all()` Hardware (fsync) nel Main Loop (Session Autosave ogni 30s)](#31-sync_all-hardware-fsync-nel-main-loop-session-autosave-ogni-30s)
   - 3.2 [Ispezione Sincrona Intensiva `/proc` e `statvfs` ogni 3s (`system_stats.rs`)](#32-ispezione-sincrona-intensiva-proc-e-statvfs-ogni-3s-system_statsrs)
   - 3.3 [Debounce e Throughput in `check_triggers()` sul Flusso Terminale](#33-debounce-e-throughput-in-check_triggers-sul-flusso-terminale)
   - 3.4 [Estrazione Testo FFI per-frame nel Rendering Cairo di VI Copy Mode](#34-estrazione-testo-ffi-per-frame-nel-rendering-cairo-di-vi-copy-mode)
   - 3.5 [I/O Sincrono nel File Browser Locale (`local_entries`)](#35-io-sincrono-nel-file-browser-locale-local_entries)
   - 3.6 [Timer di Polling Attivo a 100ms](#36-timer-di-polling-attivo-a-100ms)
4. [Database, Concorrenza & Sottosistemi Remoti](#4-database-concorrenza--sottosistemi-remoti)
   - 4.1 [SQLite History: Lock Mutex Ridondanti, Assenza Statement Cache e Full Scans](#41-sqlite-history-lock-mutex-ridondanti-assenza-statement-cache-e-full-scans)
   - 4.2 [SQLite Notes: Esecuzione DDL ad ogni Tasto, Ricompilazione Regex e Mancanza File Lock](#42-sqlite-notes-esecuzione-ddl-ad-ogni-tasto-ricompilazione-regex-e-mancanza-file-lock)
   - 4.3 [Socket IPC di Controllo e Gestione Buffer nei Broker Remoti PTY](#43-socket-ipc-di-controllo-e-gestione-buffer-nei-broker-remoti-pty)
   - 4.4 [Process Spawning Continuo in SFTP/FTP ed Assenza Cache Directory](#44-process-spawning-continuo-in-sftpftp-ed-assenza-cache-directory)
5. [Sicurezza, Soundness dei Blocchi `unsafe` & Rust Idioms](#5-sicurezza-soundness-dei-blocchi-unsafe--rust-idioms)
   - 5.1 [Soundness Bug Critico: Data Race su `Sendable<T>` e `ViewerTab`](#51-soundness-bug-critico-data-race-su-sendablet-e-viewertab)
   - 5.2 [Vulnerabilità di Sicurezza: Tool Permission Spoofing & Troncamento Comandi ACP](#52-vulnerabilità-di-sicurezza-tool-permission-spoofing--troncamento-comandi-acp)
   - 5.3 [Assenza di Memory Zeroization per Password e Chiavi TOTP](#53-assenza-di-memory-zeroization-per-password-e-chiavi-totp)
   - 5.4 [Violazioni POSIX e Memoria Non Inizializzata (`ptsname`, `gmtime_r`)](#54-violazioni-posix-e-memoria-non-inizializzata-ptsname-gmtime_r)
   - 5.5 [Limiti del Test di Isolamento Headless (`tests/headless.rs`)](#55-limiti-del-test-di-isolamento-headless-testsheadlessrs)
   - 5.6 [Tabella Risoluzione Warning Clippy](#56-tabella-risoluzione-warning-clippy)
6. [Guida Pratica alle Ottimizzazioni con Codice](#6-guida-pratica-alle-ottimizzazioni-con-codice)
7. [Matrice di Priorità & Roadmap d'Intervento](#7-matrice-di-priorità--roadmap-dintervento)
8. [Stato di Applicazione](#8-stato-di-applicazione)

---

> **Stato al 13/11/2026**: i quick win e il secondo batch sono stati applicati
> (commit `efd5447` e successivo). Il documento resta la fotografia originale;
> vedi la [sezione 8](#8-stato-di-applicazione) per cosa è stato chiuso e cosa
> resta aperto.

## 1. Panoramica Esecutiva

Oxterm è un emulatore di terminale ad alte prestazioni scritto in **Rust 2021**, basato sui binding **GTK 3.24** (`gtk-rs`) e sulla libreria terminale **VTE 0.70** (`zoha-vte`). Il codebase conta circa **55.700 righe di codice Rust**, distribuite tra il motore grafico interattivo e il layer `src/headless/` (pensato per isolare la logica priva di dipendenze X11/Wayland/GTK).

L'applicazione è ricca di funzionalità moderne: integrazione protocollo Agent Client Protocol (ACP) e Model Context Protocol (MCP), integrazione shell OSC 133, gestione di tunnel SSH e desktop remoti VNC/RDP, cronologia SQL con ricerca FTS5 trigram, modalità di copia modale in stile VI e pannelli note/file browser integrati.

Tuttavia, un'analisi approfondita condotta tramite 4 agenti specializzati ha fatto emergere **forti debiti tecnici, inefficienze prestazionali e alcune vulnerabilità di sicurezza e soundness**:

- **Monolitismo estremo**: 3 soli file concentrano oltre **17.000 righe** di codice (`window.rs` 7.456 righe, `terminal.rs` 6.651 righe, `settings_dialog.rs` 2.926 righe).
- **Blocchi del Thread GTK**: Operazioni sincrone pesanti (chiamate `fsync`/`sync_all` hardware ogni 30s per l'autosave, traversal di `/proc` e `statvfs` ogni 3s, scansioni directory locali) vengono eseguite sul thread principale della UI, causando freeze visibili e micro-stuttering.
- **Degradazione su Output Massivo**: L'estrazione e valutazione dei trigger (`check_triggers`) valuta regex sincrone su migliaia di righe ogni 80ms durante output continui (es. `cat` di file enormi o build log), allocando grandi quantità di vettori e stringhe intermedie.
- **Inefficienze Database**: La ricerca nella cronologia (`history.rs`) include una `UNION` con `REPLACE(...) LIKE ?` che azzera i benefici dell'indice FTS5 ed esegue un Full Table Scan sequenziale di 1.000.000 di righe ad ogni ricerca a termine singolo. Nel pannello note, ogni singola battitura esegue il DDL completo `init_schema` aprendo un nuovo thread OS.
- **Soundness e Sicurezza**: `Sendable<T>` e `ViewerTab` implementano `Send` e `Sync` su puntatori non thread-safe (`Rc<RefCell<...>>`) provocando violazioni di data race conclamate; l'approvazione comandi ACP è soggetta a permission spoofing; segreti e chiavi TOTP risiedono in plain text senza zeroize.

---

## 2. Anti-Pattern Architetturali & Qualità del Codice

### 2.1 I Monoliti "God Objects" (`window.rs`, `terminal.rs`, `settings_dialog.rs`)

Nel progetto si riscontra una concentrazione sproporzionata di complessità in tre file:

1. **`src/window.rs` (7.456 righe)**:
   Accorpa gestione finestre (`MainWindow`, `DetachedWindow`), splitting notebook GTK, costruzione dinamica dell'intera barra dei menu e acceleratori, telemetria di sistema, pannelli laterali (Notes, Agent, Connections), session persistence e perfino una suite di test end-to-end da 600 righe (`pub mod test_scenario`).
   La struct interna `main_imp::MainWindow` ([src/window.rs:1675-1776](file:///home/andres/oxterm/src/window.rs#L1675-L1776)) definisce **più di 40 campi** racchiusi in `RefCell` e `Cell`, rendendo lo stato fragile e aumentando il rischio di panic a runtime dovuti a prestiti multipli.
2. **`src/terminal.rs` (6.651 righe)**:
   La struct `imp::TerminalBox` ([src/terminal.rs:1116-1280](file:///home/andres/oxterm/src/terminal.rs#L1116-L1280)) conta **più di 75 campi**. Il blocco `impl TerminalBox` si estende per oltre 5.250 righe consecutive gestendo PTY locali, broker remoti, rendering Cairo di gutter e scrollbar, modalità VI, parsing OSC 133 e autocompletamento storico.
3. **`src/settings_dialog.rs` (2.926 righe)**:
   Contiene tutte le 8 pagine di configurazione, dialoghi ausiliari di Help, About e la diagnostica completa ([src/settings_dialog.rs:2311-2926](file:///home/andres/oxterm/src/settings_dialog.rs#L2311-L2926)). Tutte le pagine vengono costruite in modo **eager** all'apertura del dialogo ([src/settings_dialog.rs:1947](file:///home/andres/oxterm/src/settings_dialog.rs#L1947)), allocando centinaia di widget e strutture di ricerca anche se l'utente desidera cambiare un solo toggle.

### 2.2 Duplicazione Massiccia tra `MainWindow` e `DetachedWindow`

`DetachedWindow` ([src/window.rs:444-1594](file:///home/andres/oxterm/src/window.rs#L444-L1594)) e `MainWindow` ([src/window.rs:1828-6604](file:///home/andres/oxterm/src/window.rs#L1828-L6604)) implementano quasi specularmente la medesima logica con decine di funzioni duplicate:

- **Costruzione Menu (`build_menu`)**: [src/window.rs:812-1318](file:///home/andres/oxterm/src/window.rs#L812-L1318) (506 righe) vs [src/window.rs:2859-3620](file:///home/andres/oxterm/src/window.rs#L2859-L3620) (761 righe). Oltre **1.250 righe di codice identico** per voci di menu, check item e acceleratori da tastiera copiate e incollate.
- **Costruzione HeaderBar**: [src/window.rs:682-763](file:///home/andres/oxterm/src/window.rs#L682-L763) vs [src/window.rs:2608-2783](file:///home/andres/oxterm/src/window.rs#L2608-L2783).
- **Statistiche e Timer Hardware**: `refresh_stats`, `start_stats_timer`, `stop_stats_timer` duplicate tra righe 1456-1581 e 6228-6350.

### 2.3 Violazione SRP in `src/headless/connections.rs` e Binary Bloat in `spec.rs`

- **`src/headless/connections.rs` (2.914 righe)**: viola il principio di singola responsabilità accumulando modelli di dominio, serializzazione JSON, lock atomico filesystem con `libc::flock`, parsing configurazioni OpenSSH, monitoraggio frequenza d'uso, algoritmi di costruzione albero e ranking per la UI, pianificazione parametri SSH/Askpass, allocazione porte TCP per tunnel e 1.000 righe di test unitari.
- **`src/headless/spec.rs` (1.848 righe)**: include all'interno del runtime principale di `oxterm` l'intero motore di generazione per le man page roff, tabelle markdown e script di completamento shell per Bash, Fish e Zsh ([src/headless/spec.rs:1230-1848](file:///home/andres/oxterm/src/headless/spec.rs#L1230-L1848)). Questi generatori servono esclusivamente allo strumento di build `oxterm-docgen` e appesantiscono inutilmente il binario utente finale.

### 2.4 Anti-Pattern Modale `dialog.run()` (35+ istanze)

In più di 35 posizioni (tra cui [src/window.rs:1377, 4492, 5055, 6595](file:///home/andres/oxterm/src/window.rs#L4492), [src/settings_dialog.rs:500, 2207](file:///home/andres/oxterm/src/settings_dialog.rs#L500), [src/agent_panel.rs:1250](file:///home/andres/oxterm/src/agent_panel.rs#L1250), [src/terminal.rs:3094](file:///home/andres/oxterm/src/terminal.rs#L3094), [src/tunnel_manager.rs:379](file:///home/andres/oxterm/src/tunnel_manager.rs#L379)) viene impiegato il metodo bloccante:
```rust
let resp = dialog.run();
```
`gtk_dialog_run` avvia un nested `GMainLoop` ricorsivo che congela lo stack di esecuzione. Gli eventi che arrivano durante l'attesa vengono processati in modo rientrante, con elevato rischio di panic da borrow multiplo su `RefCell` o manipolazione di widget distrutti. GTK4 ha rimosso completamente questa API; è indispensabile migrare a `dialog.connect_response(...)`.

### 2.5 Campi Fantasma (Dead Code) in `TerminalBox`

In [src/terminal.rs:1157-1160](file:///home/andres/oxterm/src/terminal.rs#L1157-L1160):
```rust
pub resize_settle_source: RefCell<Option<glib::SourceId>>,
pub resize_nudge_pending: RefCell<bool>,
pub skip_next_resize_nudge: RefCell<bool>,
pub last_alloc_size: RefCell<(i32, i32)>,
```
Questi quattro campi vengono allocati per ciascuna scheda di terminale aperta ma non sono **mai letti né scritti** in alcuna parte del progetto (rimasuglio di logica di resize dismessa).

---

## 3. Colli di Bottiglia Prestazionali & Blocco del Thread GTK

### 3.1 `sync_all()` Hardware (fsync) nel Main Loop (Session Autosave ogni 30s)

In [src/window.rs:1845-1865](file:///home/andres/oxterm/src/window.rs#L1845-L1865), un timer GLib esegue ogni 30 secondi:
```rust
win.save_session("last");
```
La catena di chiamate procede con `save_session("last")` -> `build_session_data()` -> `session::save_session()` -> `persistence::atomic_write()`.
In [src/headless/persistence.rs:55, 87](file:///home/andres/oxterm/src/headless/persistence.rs#L55):
```rust
file.sync_all()?;
// rename atomico
File::open(parent)?.sync_all()?;
```
`sync_all()` impartisce una chiamata di sistema bloccante `fsync(2)`, costringendo il controller disco al flush fisico dei buffer su supporto non volatile. Eseguita direttamente sul thread principale GTK, questa operazione introduce pause da decine di millisecondi a diversi secondi su dischi magnetici, filesystem occupati o mount di rete (NFS/SSHFS), causando freeze periodici del terminale.

### 3.2 Ispezione Sincrona Intensiva `/proc` e `statvfs` ogni 3s (`system_stats.rs`)

Sia `MainWindow` ([src/window.rs:6279](file:///home/andres/oxterm/src/window.rs#L6279)) che `DetachedWindow` ([src/window.rs:1507](file:///home/andres/oxterm/src/window.rs#L1507)) invocano ogni 3 secondi sul main thread:
1. `update_cwd_badge`: lettura symlink `/proc/{pid}/cwd`.
2. `apply_ssh_alert`: scansione completa di `/proc/{pid}/environ` alla ricerca di marker SSH, seguita dalla scansione dei processi figli in `/proc/{pid}/task/{pid}/children` e lettura di `/proc/{child}/comm` e `cmdline`.
3. `system_stats::collect(false)`: lettura di `/proc/meminfo`, lettura di `/proc/stat` e la chiamata di sistema bloccante **`statvfs("/")`** ([src/system_stats.rs:74](file:///home/andres/oxterm/src/system_stats.rs#L74)).
4. `system_stats::collect_self()`: lettura di `/proc/{pid}/statm` e `/proc/{pid}/stat`.

La cache a 500ms presente in `system_stats.rs:97` è completamente inutile perché il timer della UI scatta ogni 3000ms: ad ogni intervallo la cache è già scaduta e tutte le decine di letture su filesystem e la `statvfs` vengono eseguite al 100% sul thread UI.

### 3.3 Debounce e Throughput in `check_triggers()` sul Flusso Terminale

In [src/terminal.rs:1688-1700](file:///home/andres/oxterm/src/terminal.rs#L1688-L1700), il segnale `contents_changed` di VTE attiva un timeout di 80ms verso `check_triggers()`.
Durante output ad alto volume (es. `cat large_file` o compilation logs):
1. **Schedulazione Continua**: Il timer scatta ininterrottamente 12.5 volte al secondo sul thread GTK.
2. **Estrazione e Copia Doppia del Testo**: In [src/terminal/text.rs:44-45](file:///home/andres/oxterm/src/terminal/text.rs#L44-L45), `zoha_vte_sys::vte_terminal_get_text_range` estrae la stringa in un puntatore C, racchiuso in `glib::GString`. Subito dopo, `text.map(|t| t.to_string())` alloca una **seconda stringa heap Rust**, duplicando interamente megabyte di testo.
3. **Allocazione Vettoriale Intermedia**: In [src/terminal.rs:2782](file:///home/andres/oxterm/src/terminal.rs#L2782), `let lines: Vec<&str> = text.split('\n').collect();` alloca un vettore per decine di migliaia di righe ad ogni tick.
4. **Valutazione Regex Sincrona**: In [src/terminal.rs:2798](file:///home/andres/oxterm/src/terminal.rs#L2798), ogni riga viene valutata contro tutti i pattern regex attivi. Con 5 trigger e 4.000 righe estratte, vengono eseguite 20.000 valutazioni regex sequenziali bloccando il main loop grafico.
5. **Bug Riga 2761**: Se non vi sono trigger né subscriber di eventi (`!want_output`), la funzione ritorna subito senza aggiornare `trigger_row`. Se un client si connette in un secondo momento, Oxterm tenterà di estrarre in un colpo solo tutte le centinaia di migliaia di righe accumulate dall'inizio della sessione.

### 3.4 Estrazione Testo FFI per-frame nel Rendering Cairo di VI Copy Mode

In [src/terminal/vi_copy.rs:694-707](file:///home/andres/oxterm/src/terminal/vi_copy.rs#L694-L707), la funzione `draw_vi_overlay` viene chiamata ad **ogni singolo frame di ridisegno** Cairo (60 fps o al movimento del cursore):
```rust
for row in (top as i64)..(top as i64 + page as i64 + 1) {
    let Some((start, end)) = self.vi_span(row) else { continue; };
    ...
}
```
`vi_span` invoca `vi_line_len(row)`, che a sua volta invoca `vi_line(row)` ([riga 68](file:///home/andres/oxterm/src/terminal/vi_copy.rs#L68)), che esegue `text::range(&self.vte(), row, 0, row, -1)`.
Per un terminale standard di 60-80 righe, ogni singolo frame di rendering Cairo effettua **60-80 chiamate FFI C a VTE e 120-160 allocazioni di stringhe sul main thread**. Questo genera lag percettibile e consumo CPU anomalo durante la selezione visiva del testo.

### 3.5 I/O Sincrono nel File Browser Locale (`local_entries`)

In [src/file_browser.rs:422-455](file:///home/andres/oxterm/src/file_browser.rs#L422-L455), la funzione `load_local` invoca `local_entries` direttamente sul thread GTK:
`std::fs::read_dir(path)` esegue per ciascun elemento syscall `stat(2)` o `lstat(2)`, formatta date con timezone locali e ordina il vettore. Su cartelle con migliaia di file o dischi NFS/CIFS, l'intera finestra di Oxterm si congela completamente.

### 3.6 Timer di Polling Attivo a 100ms

In due parti del codice la gestione asincrona è sostituita da timer di polling a 100ms che risvegliano costantemente la CPU:
- [src/settings_dialog.rs:1604-1644](file:///home/andres/oxterm/src/settings_dialog.rs#L1604-L1644): polling per `acp::probe` con `receiver.try_recv()`.
- [src/file_browser.rs:490-499](file:///home/andres/oxterm/src/file_browser.rs#L490-L499): polling per i risultati del worker remoto con `results.try_recv()`, catturando inoltre `browser` come `Rc` forte e impedendone la deallocazione.

---

## 4. Database, Concorrenza & Sottosistemi Remoti

### 4.1 SQLite History: Lock Mutex Ridondanti, Assenza Statement Cache e Full Scans

Nel gestore della cronologia [src/history.rs](file:///home/andres/oxterm/src/history.rs):
1. **Mutex Ridondanti**: `HistoryManager` possiede `conn: Mutex<Connection>` e `inserts_since_trim: Mutex<u32>` ([src/history.rs:288-291](file:///home/andres/oxterm/src/history.rs#L288-L291)), ma viene istanziato ed eseguito **esclusivamente** dentro il thread worker singolo `history_worker_loop` (riga 91). Tutti i metodi acquisiscono lock non necessari ad ogni operazione.
2. **Assenza di `prepare_cached`**: In `add_with_context` ([riga 510](file:///home/andres/oxterm/src/history.rs#L510)) e `set_command_result` ([riga 593](file:///home/andres/oxterm/src/history.rs#L593)), `conn.execute()` ricompila da zero lo statement SQL ad ogni comando eseguito nel terminale (`sqlite3_prepare_v2` -> compilazione bytecode VDBE -> `step` -> `finalize`).
3. **`GROUP BY command` su 1.000.000 di Righe**: Con `TRIM_MAX_ROWS = 1_000_000`, la query per visualizzare la cronologia vuota/recente ([righe 621-638](file:///home/andres/oxterm/src/history.rs#L621-L638)) esegue un `GROUP BY command` sull'intera tabella con calcolo di `MAX(id)`, forzando SQLite a un'aggregazione massiva in memoria.
4. **Annullamento di FTS5 in `search_fts` con `UNION` Sequenziale**: In [src/history.rs:758-772](file:///home/andres/oxterm/src/history.rs#L758-L772):
   ```rust
   if positive.len() == 1 {
       candidates.push_str(&format!(
           " UNION SELECT MAX(c3.id) FROM commands c3 WHERE REPLACE(c3.command, ' ', '') LIKE ? ESCAPE '\\' ..."
       ));
   }
   ```
   Ogni volta che l'utente cerca una singola parola (es. `git`, `cargo`, `docker`), la query FTS5 trigram (sub-millisecondo) viene unita in `UNION` con una query che esegue `REPLACE(c3.command, ' ', '') LIKE ?`. Poiché non esiste un indice sulla funzione `REPLACE`, SQLite esegue un **Full Table Scan sequenziale di un milione di righe**, con allocazione di stringhe e valutazione scalare riga per riga, portando il worker al 100% CPU.

### 4.2 SQLite Notes: Esecuzione DDL ad ogni Tasto, Ricompilazione Regex e Mancanza File Lock

Nel modulo [src/notes_index.rs](file:///home/andres/oxterm/src/notes_index.rs) e [src/notes.rs](file:///home/andres/oxterm/src/notes.rs):
1. **DDL ad Ogni Keystroke**: Ad ogni pressione di tasto nella ricerca note, `search_async` genera un thread che chiama `open()`, la quale esegue `init_schema(&conn)` ([src/notes_index.rs:264-314](file:///home/andres/oxterm/src/notes_index.rs#L264-L314)) eseguendo `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS` e 3 trigger. Quando l'utente digita rapidamente, decine di thread concorrenti tentano transazioni DDL su `sqlite_master`, scatenando errori di lock contention (`database is locked`).
2. **Ricompilazione Continua di `TAG_RE`**: In [src/notes_index.rs:318](file:///home/andres/oxterm/src/notes_index.rs#L318), `let tag_re = regex::Regex::new(...)` ricompila la regex per ogni singolo file Markdown analizzato durante il refresh.
3. **Scrittura Note senza File Lock**: In `src/notes.rs:447-488`, `append_note` scrive direttamente sul file con `O_APPEND` senza acquisire `flock(fd, LOCK_EX)`. Se due istanze o processi aggiungono note contemporaneamente, le sezioni possono interleavarsi. Inoltre, la nota scritta non aggiorna l'indice SQLite, rendendola invisibile alla ricerca fino al successivo refresh globale.

### 4.3 Socket IPC di Controllo e Gestione Buffer nei Broker Remoti PTY

1. **`src/control.rs:1371-1418`**: Il server IPC su socket Unix alloca un thread OS per ciascuna connessione (massimo 32). Tutte le richieste non banali vengono inoltrate con `crate::run_on_main` attendendo una risposta con timeout: se il main thread GTK è impegnato in un rendering pesante, i client IPC falliscono con timeout.
2. **`src/remote.rs:1152-1173` (`broadcast_output`)**: Quando il processo figlio genera output (chunk da 8 KB), il broker alloca multipli `Vec<u8>` per ciascun client connesso inserendoli in una `VecDeque<Vec<u8>>`. L'enorme frammentazione dell'allocatore glibc ha costretto gli sviluppatori a ricorrere a chiamate esplicite a `libc::malloc_trim(0)`. Quando un client lento supera `MAX_PENDING`, viene disconnesso logicamente ma la sua coda di 4 MB di buffer non viene deallocata subito.

### 4.4 Process Spawning Continuo in SFTP/FTP ed Assenza Cache Directory

In [src/headless/remote_files.rs:237-263, 372-385](file:///home/andres/oxterm/src/headless/remote_files.rs#L237-L263):
Per qualsiasi azione di navigazione remota (`list`, `home_directory`, `delete`, `rename`), viene avviato un nuovo processo `sftp` o `lftp`. Nel caso di FTP, ogni click di cartella rinegozia la connessione TCP, effettua handshake TLS e invia credenziali `USER`/`PASS` da zero. Inoltre, la cancellazione ricorsiva esegue un processo `sftp` separato per ciascun singolo file da eliminare. Non esiste alcuna cache in memoria per i listing di directory, costringendo a un nuovo round-trip anche tornando indietro alla cartella precedente.

---

## 5. Sicurezza, Soundness dei Blocchi `unsafe` & Rust Idioms

### 5.1 Soundness Bug Critico: Data Race su `Sendable<T>` e `ViewerTab`

#### A. Il wrapper `Sendable<T>`
In [src/main.rs:81-83](file:///home/andres/oxterm/src/main.rs#L81-L83):
```rust
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}
unsafe impl<T> Sync for Sendable<T> {}
```
E il suo impiego in [src/agent_panel.rs:334-348](file:///home/andres/oxterm/src/agent_panel.rs#L334-L348):
```rust
let weak = crate::Sendable(Rc::downgrade(&panel));
let _ = std::thread::Builder::new()
    .name("oxterm-agent-ui".to_string())
    .spawn(move || {
        let weak = weak;
        while let Ok(event) = rx.recv() {
            let weak = crate::Sendable(weak.0.clone()); // <-- CLONAZIONE DI RC::WEAK SU WORKER THREAD!
            crate::run_on_main(move || {
                let weak = weak;
                if let Some(panel) = weak.0.upgrade() {
                    panel.handle_event(event);
                }
            });
        }
    });
```
> [!CAUTION]
> **Data Race e Undefined Behavior certo**: `Sendable<T>` forza `Send` e `Sync` su qualsiasi tipo. `std::rc::Weak` contiene contatori di riferimento **non atomici** (`Cell<usize>`). Eseguire `weak.0.clone()` su un thread in background mentre il main thread GTK alloca o rilascia altri `Rc`/`Weak` della stessa istanza genera un accesso in scrittura concorrente non sincronizzato sul contatore del box, violando il modello di memoria di Rust (Undefined Behavior).

#### B. La struct `ViewerTab`
In [src/connections_ui.rs:1638-1667](file:///home/andres/oxterm/src/connections_ui.rs#L1638-L1667):
```rust
struct ViewerTab(Rc<RefCell<Option<TerminalBox>>>);
unsafe impl Send for ViewerTab {}
unsafe impl Sync for ViewerTab {}
```
`RefCell` memorizza il contatore di prestiti in un `Cell<isize>` non atomico. Condividere `&ViewerTab` tra thread concorrenti aggira interamente le garanzie del borrow checker e porta a corruzione di memoria.

### 5.2 Vulnerabilità di Sicurezza: Tool Permission Spoofing & Troncamento Comandi ACP

1. **Permission Title Spoofing**: In [src/headless/acp.rs:415-422](file:///home/andres/oxterm/src/headless/acp.rs#L415-L422) e [src/agent_panel.rs:1240](file:///home/andres/oxterm/src/agent_panel.rs#L1240), il titolo visualizzato nel messaggio di autorizzazione ("Allow the agent to run this tool?\n\n{title}") viene prelevato direttamente dal payload JSON-RPC dell'agente esterno (`/toolCall/title`). Un agente malevolo può fornire un titolo innocuo come `"Check git status"` mentre il tool reale esegue `terminal_send` con comandi distruttivi (`rm -rf /`).
2. **Troncamento della Testa in `truncate_head`**: In [src/headless/agent.rs:257-268, 494-502](file:///home/andres/oxterm/src/headless/agent.rs#L257-L268), se un comando supera 600 caratteri, `truncate_head` **elimina i caratteri iniziali** della stringa mostrando solo la parte finale. Un comando costruito ad arte con `rm -rf / #` seguito da 600 caratteri innocui nasconde all'utente l'azione pericolosa inducendolo ad approvarla.

### 5.3 Assenza di Memory Zeroization per Password e Chiavi TOTP

In [src/headless/askpass.rs:164-186](file:///home/andres/oxterm/src/headless/askpass.rs#L164-L186), [src/bin/oxterm-askpass.rs:64-102](file:///home/andres/oxterm/src/bin/oxterm-askpass.rs#L64-L102) e [src/headless/totp.rs:83-163](file:///home/andres/oxterm/src/headless/totp.rs#L83-L163), password, chiavi segrete e buffer HMAC vengono memorizzati in normali `String` e `Vec<u8>`. Al de-scope, la memoria non viene sovrascritta con zeri (`zeroize`), lasciando i segreti leggibili nelle free list dell'allocatore heap, nei file di core dump o nello swap di sistema.

### 5.4 Violazioni POSIX e Memoria Non Inizializzata (`ptsname`, `gmtime_r`)

1. **`libc::ptsname` non thread-safe**: In [src/terminal.rs:707](file:///home/andres/oxterm/src/terminal.rs#L707) e [src/remote.rs:2005](file:///home/andres/oxterm/src/remote.rs#L2005), `libc::ptsname(master)` restituisce un puntatore a un buffer statico condiviso interno a glibc. L'apertura contemporanea di terminali su thread diversi sovrascrive il buffer causando l'apertura del PTY slave errato. È necessario usare la variante reentrante `libc::ptsname_r`.
2. **`MaybeUninit` non verificato in `src/logging.rs:58-67`**: I valori di ritorno di `libc::gettimeofday` e `libc::gmtime_r` non vengono controllati prima di eseguire `assume_init()`. Se una funzione fallisce ritornando NULL o errore, si legge memoria non inizializzata (Undefined Behavior).
3. **Casting di `sysconf` a `u64` in `src/system_stats.rs:161`**: `libc::sysconf(libc::_SC_PAGESIZE)` restituisce `c_long` (`-1` in caso di errore). Il cast incondizionato `-1i64 as u64` produce `u64::MAX`, portando a overflow numerico immediato nella moltiplicazione successiva.

### 5.5 Limiti del Test di Isolamento Headless (`tests/headless.rs`)

Il test architetturale `headless_sources_do_not_reach_into_the_gui` ([tests/headless.rs:13-48](file:///home/andres/oxterm/tests/headless.rs#L13-L48)) scansiona i file con una blacklist fissa di 8 stringhe. Il test soffre di gravi debolezze:
- Non intercetta import multilinea (es. `use crate::{\n terminal::X\n};`).
- Non include moduli GUI come `agent_panel`, `connections_ui`, `file_browser`, `notes_ui`, né librerie grafiche correlate (`cairo::`, `pango::`, `gdk_pixbuf::`).
- *Soluzione ideale*: estrarre `src/headless/` in un crate indipendente `crates/oxterm-core` in un Cargo workspace, delegando al compilatore la garanzia formale di assenza di dipendenze GUI.

### 5.6 Tabella Risoluzione Warning Clippy

L'ispezione con `cargo clippy --all-targets` ha evidenziato i seguenti warning da risolvere:

| File e Linee | Warning Clippy | Causa | Correzione |
|---|---|---|---|
| `src/headless/agent.rs:353, 401` | `too_many_arguments` | Funzioni `audit_line` e `audit` con 8 argomenti | Raggruppare in struct `AuditEvent<'a>` |
| `src/headless/totp.rs:28, 30, 61` | `chunks_exact_to_as_chunks` | `chunks_exact(BLOCK)` con dimensione costante | Sostituire con `as_chunks::<BLOCK>().0` |
| `src/headless/events.rs:155` | `unnecessary_map_or` | `.map_or(true, \|e\| ...)` | Sostituire con `.is_none_or(\|e\| ...)` |
| `src/headless/spec.rs:1307` | `no_effect_replace` | `.replace(' ', " ")` | Sostituzione di uno spazio con sé stesso; rimuovere |
| `src/headless/spec.rs:1631` | `single_char_add_str` | `out.push_str("\"")` | Sostituire con `out.push('\"')` |
| `src/headless/ssh_config.rs:366` | `manual_div_ceil` | `(data.len() + 2) / 3 * 4` | Sostituire con `data.len().div_ceil(3) * 4` |
| `src/headless/agent.rs:868` | `manual_str_repeat` | `std::iter::repeat('x').take(n).collect()` | Sostituire con `"x".repeat(n)` |
| `src/terminal.rs:755` | `manual_c_str_literals` | `b"vte_...\\0".as_ptr().cast()` | Sostituire con `c"vte_terminal_set_font_options".as_ptr()` |
| `src/terminal.rs:463` | `manual_pattern_char_comparison` | `trim_end_matches(\|c\| matches!(...))` | Sostituire con array `trim_end_matches([',', ';', '.'])` |
| `src/terminal.rs:4326` | `explicit_counter_loop` | Contatore manuale in loop `char_indices` | Sostituire con `.char_indices().enumerate()` |
| `src/notes.rs:492` & `settings.rs:566` | `items_after_test_module` | Funzioni di produzione collocate dopo `mod tests` | Spostare le funzioni prima del modulo di test |
| `src/history.rs:219` | `type_complexity` | Tipo complesso in `OnceLock<Mutex<HashMap<...>>>` | Definire un type alias esplicito |

---

## 6. Guida Pratica alle Ottimizzazioni con Codice

### 6.1 `src/system_stats.rs`: Worker Asincrono Disaccoppiato e Zero-Allocations

```rust
// Disaccoppiamento completo: il thread GTK legge solo uno snapshot in memoria protetto da RwLock.
// Zero chiamate statvfs e zero letture /proc sul thread della UI.

use std::sync::{OnceLock, RwLock};
use std::time::Duration;
use std::io::Read;

#[derive(Clone, Default)]
pub struct StatsSnapshot {
    pub system_stats: String,
    pub self_stats: String,
}

static STATS_CACHE: OnceLock<RwLock<StatsSnapshot>> = OnceLock::new();

fn cache() -> &'static RwLock<StatsSnapshot> {
    STATS_CACHE.get_or_init(|| {
        let storage = RwLock::new(StatsSnapshot::default());
        std::thread::Builder::new()
            .name("oxterm-stats-worker".into())
            .spawn(|| stats_worker_loop())
            .expect("spawn stats worker");
        storage
    })
}

pub fn collect(is_ssh: bool) -> String {
    if is_ssh {
        return ssh_placeholder();
    }
    cache().read().unwrap().system_stats.clone()
}

pub fn collect_self() -> String {
    cache().read().unwrap().self_stats.clone()
}

fn stats_worker_loop() {
    let mut prev_cpu = None;
    let mut prev_proc = None;
    let mut buf = [0u8; 4096];

    loop {
        let (mem_used, mem_total) = read_meminfo_fast(&mut buf);
        let cpu = read_cpu_percent(&mut prev_cpu, &mut buf);
        let (disk_used, disk_total) = disk_usage("/");

        let mem_pct = if mem_total > 0 { (mem_used * 100) / mem_total } else { 0 };
        let disk_pct = if disk_total > 0 { (disk_used * 100) / disk_total } else { 0 };

        let sys = format!(
            "  CPU {:5.1}%  RAM {}/{} ({}%)  Disk {}/{} ({}%)",
            cpu, mb(mem_used), mb(mem_total), mem_pct, mb(disk_used), mb(disk_total), disk_pct
        );

        let pid = std::process::id();
        let rss = read_proc_rss(pid, &mut buf);
        let self_cpu = read_process_cpu_percent(pid, &mut prev_proc, &mut buf);
        let own = format!("Oxterm  CPU {:5.1}%  RAM {}  ", self_cpu, mb(rss));

        if let Ok(mut lock) = cache().write() {
            lock.system_stats = sys;
            lock.self_stats = own;
        }

        std::thread::sleep(Duration::from_secs(2));
    }
}
```

### 6.2 `src/terminal.rs`: `check_triggers()` Efficiente e Privo di Allocazioni Vettoriali

```rust
// Gate precoce su contents_changed (src/terminal.rs:1688):
vte.connect_contents_changed(move |_| {
    if active_triggers().is_empty() && !crate::headless::events::bus().wants_output() {
        return;
    }
    if pending.replace(true) {
        return;
    }
    let pending = pending.clone();
    let weak = weak.clone();
    glib::timeout_add_local_once(Duration::from_millis(80), move || {
        pending.set(false);
        if let Some(term) = weak.upgrade() {
            term.check_triggers();
        }
    });
});

// Implementazione di check_triggers senza collect intermedio e con lookback cap
fn check_triggers(&self) {
    let triggers = active_triggers();
    let want_output = crate::headless::events::bus().wants_output();
    let vte = self.vte();
    let (column, row) = vte.cursor_position();

    if triggers.is_empty() && !want_output {
        self.imp().trigger_row.set(row);
        return;
    }

    let last_row = self.imp().trigger_row.get();
    let start = if last_row < 0 {
        (row - 200).max(0)
    } else if row < last_row {
        self.imp().output_row.set(row - 1);
        row
    } else {
        const MAX_BURST_LOOKBACK: i64 = 500;
        if row - last_row > MAX_BURST_LOOKBACK {
            row - MAX_BURST_LOOKBACK
        } else {
            last_row
        }
    };

    let Some(text) = text::range(&vte, start, 0, row, column) else {
        self.imp().trigger_row.set(row);
        return;
    };

    let in_command_output = self.command_running();
    let profile = settings().get_str("active_profile");
    let profile_trimmed = profile.trim();
    let active_profile = if profile_trimmed.is_empty() { None } else { Some(profile_trimmed) };

    // Iterazione diretta su lines() senza allocare Vec<&str>
    for (offset, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let line_row = start + offset as i64;
        if want_output && line_row > self.imp().output_row.get() {
            self.publish_event("output_line", json!({"line": line, "row": line_row}));
            self.imp().output_row.set(line_row);
        }
        if triggers.is_empty() {
            continue;
        }
        let hits = triggers.evaluate(
            line,
            crate::headless::triggers::Context {
                profile: active_profile,
                in_command_output,
            },
        );
        for hit in hits {
            if self.imp().trigger_disabled.borrow().contains(hit.id.as_str()) {
                continue;
            }
            let key = (line_row, hit.id.clone());
            if self.imp().trigger_fired.borrow().contains(&key) {
                continue;
            }
            self.imp().trigger_fired.borrow_mut().insert(key);
            if hit.once {
                self.imp().trigger_disabled.borrow_mut().insert(hit.id);
            }
            self.apply_trigger_effects(&hit.effects);
        }
    }
    self.imp().trigger_row.set(row);
    let cutoff = row - 200;
    self.imp().trigger_fired.borrow_mut().retain(|(r, _)| *r >= cutoff);
}
```

### 6.3 `src/history.rs`: Rimozione Mutex, `prepare_cached` ed Eliminazione Full Table Scan

```rust
// HistoryManager posseduto direttamente dal worker thread senza Mutex
pub struct HistoryManager {
    conn: Connection,
    inserts_since_trim: u32,
}

impl HistoryManager {
    pub fn add_with_context(
        &mut self,
        command: &str,
        cwd: &str,
        exit_code: i64,
        duration_ms: Option<i64>,
        git_branch: Option<&str>,
    ) -> i64 {
        let ts = Self::now_ts();
        // Utilizzo dello statement cache per evitare ricompilazioni continue
        let res = self.conn.prepare_cached(
            "INSERT INTO commands (command, cwd, exit_code, timestamp, duration_ms, git_branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        ).and_then(|mut stmt| {
            stmt.execute(params![command, cwd, exit_code, ts, duration_ms, git_branch])
        });

        match res {
            Ok(_) => {
                let id = self.conn.last_insert_rowid();
                self.bump_inserts(1);
                id
            }
            Err(e) => {
                crate::logging::log_warning(&format!("could not add history entry: {e}"));
                0
            }
        }
    }
}

// In search_fts: rimozione della UNION sequenziale su 1M di record
// Si sfrutta unicamente l'indice FTS5 trigram:
let candidates = format!(
    "SELECT MAX(c2.id) FROM commands c2 \
     JOIN commands_fts f ON f.rowid = c2.id \
     WHERE {} GROUP BY c2.command",
    subquery.join(" AND ")
);
```

### 6.4 `src/status_bar.rs`: Eliminazione del Timer Leak e Thread Zombie

```rust
// Interrompe il timer GLib quando la label associata viene distrutta
let refresh = std::rc::Rc::new(move || {
    let Some(_alive_label) = weak.upgrade() else {
        return glib::ControlFlow::Break; // Ferma il timer ed elimina il leak
    };

    if running.swap(true, Ordering::SeqCst) {
        return glib::ControlFlow::Continue;
    }
    // Esecuzione comando...
    glib::ControlFlow::Continue
});
```

### 6.5 Eliminazione di `Sendable<T>` con Canale Nativo GLib

```rust
// Invece di spawnare un thread con loop mpsc::recv e clonare Rc::Weak su worker thread:
let (sender, receiver) = glib::MainContext::channel(glib::Priority::default());
let weak_panel = Rc::downgrade(&panel);

receiver.attach(None, move |event| {
    if let Some(panel) = weak_panel.upgrade() {
        panel.handle_event(event);
    }
    glib::ControlFlow::Continue
});

// I session thread dell'agente inviano gli eventi direttamente su `sender.send(event)`
```

---

## 7. Matrice di Priorità & Roadmap d'Intervento

| Priorità | Area / Componente | File Coinvolti | Tipologia Intervento | Guadagno Atteso |
|:---:|---|---|---|---|
| **P0** | **Thread Safety & Soundness** | `src/main.rs`, `src/agent_panel.rs`, `src/connections_ui.rs` | Eliminare `Sendable<T>` e `ViewerTab` con `unsafe impl Send/Sync` su `Rc`/`RefCell`; usare `glib::MainContext::channel`. | Eliminazione di data race non sincronizzati e crash di memoria casuali. |
| **P0** | **Sicurezza ACP & Askpass** | `src/headless/acp.rs`, `src/headless/agent.rs`, `src/agent_panel.rs` | Risolvere permission title spoofing; sostituire `truncate_head` con visualizzazione sicura del comando. Introdurre `Zeroizing` per password. | Prevenzione di esecuzione comandi dannosi camuffati e leak credenziali in memoria. |
| **P0** | **Autosave Sessione & Fsync** | `src/window.rs`, `src/headless/persistence.rs` | Spostare `save_session` con `sync_all` dal main thread GTK a un worker asincrono. | Eliminazione completa dei freeze dell'interfaccia ogni 30 secondi. |
| **P0** | **Resource Leak StatusBar** | `src/status_bar.rs` | Aggiungere check su `weak.upgrade()` per ritornare `glib::ControlFlow::Break`. | Eliminazione di thread OS e processi shell `/bin/sh` orfani infiniti. |
| **P0** | **Ottimizzazione SQLite History** | `src/history.rs` | Rimuovere `UNION` con `REPLACE(...) LIKE ?` in `search_fts`; adottare `prepare_cached`. | Ricerca cronologia da centinaia di millisecondi a < 2 ms (-99% CPU). |
| **P1** | **Core Loop `check_triggers()`** | `src/terminal.rs`, `src/terminal/text.rs` | Gate su `contents_changed`, cap lookback a 500 righe, iterazione zero-alloc, evitare copia doppia `GString` -> `String`. | Terminale reattivo e fluido anche durante carichi intensivi (`cat`, build massicce). |
| **P1** | **Disaccoppiamento Statistiche** | `src/system_stats.rs`, `src/window.rs` | Worker thread asincrono per `/proc` e `statvfs`; lettura istantanea in memoria dal thread GTK. | Zero latenza al cambio tab e rimozione micro-scatti ogni 3 secondi. |
| **P1** | **Rendering Cairo VI Copy** | `src/terminal/vi_copy.rs` | Eliminare l'estrazione testo VTE FFI per ogni frame e ogni riga durante il draw. | Riduzione del 90% di chiamate FFI e allocazioni a 60 fps in visual mode. |
| **P1** | **SQLite Notes & DDL** | `src/notes_index.rs`, `src/notes.rs` | Eseguire `init_schema` una sola volta con `Once`; precompilare `TAG_RE` con `LazyLock`; aggiungere `flock` su `append_note`. | Rimozione degli errori `database is locked` e velocizzazione della ricerca note. |
| **P1** | **Buffer Broker Remoto** | `src/remote.rs` | Ridurre le allocazioni `Vec<u8>` in `broadcast_output`; svuotare le code dei client lenti disconnessi. | Drastica riduzione del footprint RAM e dell'uso forzato di `malloc_trim`. |
| **P2** | **Decomposizione Monoliti** | `src/window.rs`, `src/terminal.rs`, `src/headless/connections.rs` | Suddividere i monoliti in moduli dedicati (`src/window/`, `src/terminal/`, `src/headless/connections/`); unificare i menu tra `MainWindow` e `DetachedWindow`. | Manutenibilità, modularità, riduzione di oltre 1.200 righe duplicate. |
| **P2** | **Migrazione da `dialog.run()`** | `window.rs`, `settings_dialog.rs`, `agent_panel.rs`, ecc. | Sostituire 35+ chiamate bloccanti a `dialog.run()` con `connect_response` asincrono. | Eliminazione di loop GTK nidificati e rientranza rischiosa. |
| **P2** | **Asincronicità File Browser** | `src/file_browser.rs`, `src/headless/remote_files.rs` | Spostare `local_entries` su thread worker; disconnettere `TreeModel` durante il riempimento; sessioni SFTP persistenti e LRU cache. | Eliminazione dei freeze navigando cartelle con migliaia di file o risorse remote. |
| **P3** | **Modernizzazione Idiomi & Clippy** | Crate intero | Risolvere tutti i warning Clippy (`too_many_arguments`, `chunks_exact_to_as_chunks`, `manual_c_str_literals`, `unnecessary_map_or`). | Codice pulito, idiomatico Rust 2021, zero warning del compilatore. |

---
*Report redatto per la revisione del progetto Oxterm.*

---

## 8. Stato di Applicazione

### Chiuso

| Intervento | Dove | Note |
|---|---|---|
| Campi morti di `TerminalBox` | `src/terminal.rs` | Rimossi i 4 campi di resize mai usati. |
| `ptsname` non reentrant | `src/terminal.rs`, `src/remote.rs` | Passaggio a `ptsname_r` con buffer locale. |
| Risultati syscall non controllati | `src/logging.rs`, `src/system_stats.rs` | `gettimeofday`/`gmtime_r` controllati con fallback; `sysconf` non può più diventare `u64::MAX`. |
| Timer della status bar | `src/status_bar.rs` | `Break` quando la label è stata distrutta; niente più comandi orfani. |
| Regex dei tag ricompilata | `src/notes_index.rs` | `LazyLock` + `PRAGMA user_version` per non rieseguire il DDL a ogni connessione. |
| Ricerca note a ogni tasto | `src/window.rs`, `src/notes_index.rs` | Debounce 180 ms, generation counter e schema inizializzato una volta. |
| `fsync` dell'autosave sul main thread | `src/window.rs`, `src/session.rs` | Scrittura su worker, con ordinamento dei writer per non sovrascrivere un salvataggio più recente. |
| Statement SQLite ricompilati | `src/history.rs` | `prepare_cached` su insert/update. |
| Comandi lunghi nel dialogo ACP | `src/headless/agent.rs` | Mostra testa e coda (`truncate_middle`) invece dei soli primi 600 caratteri. |
| `Sendable<Rc::Weak>` in `agent_panel` | `src/agent_panel.rs` | Canale asincrono drenato dal main context: il `Weak` non lascia più il thread GTK. |
| `unsafe impl Send/Sync` su `ViewerTab` | `src/connections_ui.rs` | Rimossi; il passaggio al main thread usa `Sendable`. |
| `/proc` e `statvfs` ogni 3 s | `src/system_stats.rs` | Worker thread + snapshot `RwLock`; la UI legge solo memoria. |
| `check_triggers` | `src/terminal.rs` | Gate su `contents_changed`, iterazione senza `Vec`, cap del burst a 2000 righe e marker aggiornato anche quando è inattivo. |
| Listing locale del file browser | `src/file_browser.rs` | `read_dir`/`stat` su worker con scarto dei risultati obsoleti. |
| Warning Clippy | crate intero | `cargo clippy --all-targets` pulito. |

### Ancora aperto

| Intervento | Perché è rimasto aperto |
|---|---|
| Permission title spoofing ACP | Il titolo è fornito dall'agente: il protocollo non permette una verifica indipendente. Si può solo mostrare più contesto, non eliminare il rischio. |
| `Zeroizing` per password/TOTP | Richiede una dipendenza nuova e tocca il flusso keyring/askpass; impatto pratico limitato a core dump e swap. |
| `UNION` con `REPLACE(...) LIKE` in `search_fts` | È una feature voluta (`gitstatus` trova `git status`). Servirebbe una colonna compatta indicizzata o una decisione di prodotto. |
| Cache del testo in `vi_copy` | Refactor del rendering per-frame, da fare con test visivi dedicati. |
| Sessioni SFTP/FTP persistenti e cache directory | Refactor medio del sottosistema remoto. |
| Migrazione da `dialog.run()` | 40 chiamate: intervento ampio e con rischio di regressioni UX. |
| Decomposizione dei monoliti | Refactor strutturale, non un quick win. |
| Generatori docgen fuori dal binario | Guadagno di dimensione marginale; feature-gating da valutare. |

