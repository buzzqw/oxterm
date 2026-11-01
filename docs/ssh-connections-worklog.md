# Gestore connessioni SSH: stato del lavoro

Documento di passaggio per chi prosegue, persona o agente AI. Aggiornalo a ogni
sessione: in fondo c'è un registro. Ultimo aggiornamento: 2026-11-01.

- **Branch:** `feat/ssh-connections`, partito da `master` a `fd0cc01`.
- **Worktree di sviluppo:** `/home/andres/oxterm-connections`, eliminato dopo il merge.
- **Proposta iniziale:** <https://claude.ai/artifact/LQG2ZPbuyUYrmMkxrPTZE4>
- **Regole del repository:** `AGENTS.md` e `CONTRIBUTING.md`. In particolare, la
  logica senza display sta in `src/headless/` e dopo aver cambiato
  `src/headless/spec.rs` va eseguito `cargo run --bin oxterm-docgen`.

## Decisioni di progetto

Non vanno riaperte senza un motivo nuovo.

1. **OpenSSH è l'unico motore.** Oxterm esegue `ssh` di sistema. Niente client
   interno come `russh`.
2. **`~/.ssh/config` si legge e non si scrive mai.** Un test lo verifica.
3. **Nessun segreto nei file di Oxterm.** Le password stanno nel Secret Service,
   tramite `secret-tool`. Niente crittografia scritta da noi.
4. **Formato pubblico.** `connections.json` sarà letto anche dalla futura TUI. I
   campi sconosciuti sopravvivono alla riscrittura (`#[serde(flatten)] unknown`).
5. **File danneggiato o più recente: mai sovrascritto.** Il primo dà errore, il
   secondo si apre in sola lettura (`version > CURRENT_VERSION`).
6. **L'helper delle password usa una lista di consenso.** Risponde solo a
   `user@host's password:` o `(user@host) Password:` della destinazione, una sola
   volta per processo `ssh`. Tutto il resto va a `/dev/tty`. Non aggiungere prompt
   generici: un `Password:` può arrivare da un bastion.
7. **Le connessioni non ripartono da sole.** Dopo un riavvio Oxterm chiede conferma.
8. **La TUI è fuori da questo lavoro.** Quando partirà, il nucleo si estrae in un
   crate e i pannelli incorporati useranno `libghostty-vt`, preferenza espressa
   dall'autore. Oxterm non dipende da PCM, né PCM da Oxterm.

## Architettura

| File | Ruolo | GTK |
| --- | --- | --- |
| `src/headless/connections.rs` | Rubrica, validazione, argomenti `ssh`, unione con `ssh_config`, albero, ricerca, import | No |
| `src/headless/tunnels.rs` | Processi `ssh -N` in background e il loro stato reale | No |
| `src/headless/askpass.rs` | Decisione sui prompt, tentativi, `secret-tool` | No |
| `src/headless/persistence.rs` | Spostato da `src/` per renderlo riutilizzabile | No |
| `src/headless/ssh_config.rs` | Aggiunto `effective_config`, un `ssh -G` con timeout | No |
| `src/bin/oxterm-askpass.rs` | Helper `SSH_ASKPASS` | No |
| `src/headless/control_cli.rs` | Parsing di `connect`, `connections`, `tunnel` | No |
| `src/control.rs` | Lato client dei comandi e operazione `tunnel` | Sì |
| `src/connections_panel.rs` | Widget del pannello | Sì |
| `src/connections_form.rs` | Dialogo di modifica | Sì |
| `src/connections_ui.rs` | Comportamento su `MainWindow`: connessione, menu, tunnel | Sì |

`src/window.rs` contiene solo lo slot del pannello e pochi collegamenti: il resto
va tenuto fuori da lì, perché il file supera già le 6.500 righe.

Flusso di apertura:

1. `connect_id` o `connect_text` trovano la voce.
2. Un thread esegue `connections::prepare_session`, con `ssh -G` limitato a 2 s.
3. `finish_connect`, sul thread GUI, apre il terminale con `add_new_tab`, associa
   id e nome con `set_connection` e fissa il titolo con `pin_tab_title`.
4. Registra l'uso in un thread e avvia i tunnel con `autostart`.

La CLI calcola il comando nel client, perché le richieste di controllo sono servite
sul thread GUI, che non deve aspettare `ssh -G`.

## Aggiornamento: opzioni portate da PCM

Su richiesta dell'autore oxterm ha ora un menu **SSH** a destra di **Notes** e le opzioni
SSH di PCM: X11, compressione, agent forwarding, keepalive, timeout, politica host
sconosciuti, algoritmi legacy, `TERM`, condivisione disattivabile; comando locale
pre-connessione, Wake-on-LAN, riconnessione, log di sessione (scritto dal broker), macro,
regole expect, TOTP; font, scrollback ed encoding per voce; apertura di un intero gruppo;
`ssh-copy-id` e `ssh-keygen` in un tab. Un tab staccato ora può tornare nella finestra
principale. Moduli nuovi: `headless/totp.rs`, `headless/expect.rs`, `headless/preconnect.rs`.

**Non portati da PCM:** profili credenziali, template, integrazione KeePassXC (il Secret
Service può già essere KeePassXC), generatore di password, pannelli di monitoraggio
(CPU, processi, disco, rete, log), pallini di raggiungibilità, audit log, sincronizzazione
di cartelle e coda con percentuale nel browser, editor remoto.

## Aggiornamento: connettori e browser file

Aggiunti `Protocol` su `Connection`, `headless/desktop.rs` (viewer e script dei server VNC)
e `headless/remote_files.rs` con `src/file_browser.rs`. Scelte da mantenere:

- **Nessun client incorporato:** i desktop sono programmi esterni, come ssh. Il browser usa
  `sftp` e `lftp` di sistema, con uno script su stdin per operazione.
- **Server VNC solo su loopback, dietro tunnel ssh in un tab.** Il tab rende visibili errori e
  domande; il `-t` fa morire il server con il terminale. Pronto significa banner `RFB`, non porta aperta.
- **Password monouso:** generata qui, digitata con echo spento tramite una regola expect sul
  prompt `OXTERM-VNC-PASSWORD:`, al viewer TigerVNC con `VNC_PASSWORD` nell'ambiente (TigerVNC
  1.16 non ha `-autopass`), ai client RDP su stdin. Mai negli argomenti.
- **wayvnc senza password:** limite noto, documentato nel manuale.
- **Import:** comando libero, comando server personalizzato e cartella condivisa chiedono consenso.

**Da sapere per le prove:** senza window manager `Tab` non esce da un combo box, e con xfwm4
`Alt+H` è catturato dal window manager: `smoke-connections.sh` va eseguito senza window manager,
come in CI. Le prove reali richiedono `OXTERM_E2E=1` (`tests/remote_files.rs`, con `sshd`, `lftp`
e `python-pyftpdlib`). Provato dal vivo: x11vnc su `debminis`, x0vncserver locale, SFTP e FTP.
Non provati dal vivo: wayvnc, desktop virtuali, TurboVNC, TightVNC, RDP, SPICE, mosh, kubectl, AWS.

## Giro di ottimizzazione

Confronto a build alternate, mediana di 3 corse, sullo stesso display (`/proc`, PSS). Nel
percorso comune cambia poco: PSS GUI 19,5 -> 18,7 MB, CPU a riposo e sotto output invariate
(non misurabili con più precisione di 10 ms). Il profilo dello stack non è disponibile senza
privilegi (`ptrace_scope`), quindi gli interventi vengono dalla lettura del codice:

| Dove | Prima | Dopo |
| --- | --- | --- |
| `alert_host`, per ogni riga del pannello e cambio titolo | Regex compilata a ogni chiamata: 191 µs | Cache: 0,3 µs |
| Ricerca nel pannello | Rilettura di rubrica, `~/.ssh/config` e stato a ogni tasto | Filtra i dati già letti |
| Avvio | Il menu SSH leggeva quei file a ogni avvio | Solo quando il menu si apre |
| Browser file aperto a riposo | Timer a 10 Hz | Nessun risveglio senza lavori in corso |
| Tunnel e viewer attivi | 10 risvegli al secondo per ciascuno | 2 dopo l'avvio |
| Browser, cancellazione di cartelle | Un `ssh -G` per ogni operazione | Riusato per 30 s |
| Tab con colori propri | Caricava tutto il catalogo temi | Solo se il tema non è tra quelli base |

**Non cambiato, da valutare:** ogni tasto interroga il broker su un socket (`is_echo_on`), per
non registrare password. Una connessione persistente ridurrebbe il lavoro, ma a velocità di
battitura umana il guadagno non si misura.

## Stato delle fasi

Tutte completate.

| Fase | Contenuto | Stato |
| --- | --- | --- |
| 0 | `ssh -G` fuori dal thread GUI, verifica dell'integrazione remota | Fatto |
| 1 | Nucleo, `connections.json`, pannello, ricerca, connessione | Fatto |
| 2 | Form, anteprima comando, CLI, import ed export | Fatto |
| 3 | Tunnel salvati con stato reale | Fatto |
| 4 | Helper askpass e Secret Service, verificati con login reale | Fatto |
| 5 | Riapertura con conferma, posizione e ordine; colori per connessione | Fatto |
| Extra | Palette, `/ssh`, conteggi su tutte le finestre, spostamento tra processi | Fatto |

## Da fare

Niente del piano resta aperto. Restano due punti che richiedono una scelta.

- [ ] **Decisione dell'autore: cronologia e notifiche per i comandi remoti.** Oggi Oxterm
      legge gli eventi solo dalla FIFO locale, apposta per non fidarsi dell'output.
      Un canale remoto richiede un progetto separato, e l'host remoto potrebbe
      falsificare gli eventi.
- [ ] **Facoltativo: voce spuria nella cronologia.** Dopo `reset` si è vista una riga
      con il testo del prompt, perché `get_real_command_text` legge dallo schermo con
      un'ancora ormai superata. È innocua e riguarda solo i tab senza OSC 133.

**Scelta volutamente non fatta:** il pannello nelle finestre separate. Non hanno tab
in cui aprire una connessione, come già vale per il picker SSH. La palette non vi
mostra le connessioni (`supports_connections`).

## Problemi trovati nel codice esistente

| Problema | Esito |
| --- | --- |
| `ssh -G` sincrono nel thread GUI | Corretto: thread e timeout di 2 s |
| `remote_attach` si blocca se `tmux_prefix_key` non è `b` | Corretto: il test ora usa una `HOME` temporanea |
| `remote_attach` lascia socket in `~/.config/oxterm/remote` se interrotto | Corretto per il futuro; restano 16 socket `oxterm-{attach,move}-*` di vecchie esecuzioni, innocui e cancellabili a mano |
| L'avviso di produzione guarda solo l'indirizzo | Corretto: per le connessioni salvate guarda anche il nome |
| Il titolo remoto sostituisce il nome della connessione | Corretto con `pin_tab_title` |
| Il controllo AUR compila il tag `v1.1.19`, che non contiene `oxterm-broker`, e fallisce anche su `master` dal commit `5f49453` | Corretto: la CI costruisce il commit corrente con la stessa ricetta |
| I comandi `/...` digitati al prompt finivano alla shell dal 2026-09-12 (`6317cea`): readline spegne `ECHO` e il controllo lo scambiava per un prompt di password. Senza OSC 133 si perdeva anche la cronologia | Corretto con `input_is_visible`: conta come visibile `ECHO`, oppure la modalità raw con la shell in primo piano. `ssh` è raw ma è un processo in primo piano, quindi una password remota resta fuori. Verificato: `read -s` e un programma raw non finiscono su disco |
| Chiudere un tab senza integrazione shell chiudeva il descrittore 0 del processo (`osc133_rfd` valeva 0 di default, dal commit `f6ad633`): il terminale successivo poteva nascere senza `stdin` | Corretto: valore iniziale -1, e `main` garantisce che i descrittori 0-2 esistano. Trovato dal test di riconnessione |
| **Detach Tab** era a senso unico | Corretto: **Return to Main Window**, **Bring Back** nel menu Tabs, voce in palette |
| Un programma che termina subito poteva chiudere il broker prima che oxterm vedesse il socket: avvio segnalato come fallito e riconnessione persa | Corretto in `spawn_broker`: un broker terminato con successo conta come sessione conclusa |
| `oxterm capture` usava solo l'adjustment di VTE, che resta indietro rispetto all'output: mancava il testo finale, e `control_api` falliva 6 volte su 8 su questa macchina, anche prima di questo lavoro | Corretto: la fine del testo tiene conto del cursore. 0 fallimenti su 8 |
| Lo script di integrazione remota non attiva cronologia e notifiche | Non corretto: serve una decisione, comportamento documentato nel manuale |
| Avvisi `clippy` su `terminal.rs`, `vi.rs`, `history.rs` e altri | Non toccati: sono di stile, e l'autore sta cambiando quei file per le prestazioni |
| `OpenSSH` usa la home di `passwd`, `dirs::home_dir` usa `$HOME` | Solo nota: con `HOME` diversa i due vedono `~/.ssh/config` diversi. Nei test usare `-F` |

## Come verificare

```bash
cargo test --locked --all-targets
cargo clippy --locked --all-targets
cargo fmt --check
cargo run --locked --bin oxterm-docgen -- --check

# Test con finestre reali, su un display virtuale:
Xvnc :77 -geometry 1400x900 -depth 24 -SecurityTypes None -rfbport 5977 -localhost &
DISPLAY=:77 OXTERM_GUI_TESTS=1 cargo test --locked \
  --test connections --test control_api --test gui_lifecycle -- --test-threads=1
DISPLAY=:77 ./scripts/smoke-connections.sh
scripts/e2e/password-login.sh
```

Non usare `pkill -f` con un percorso del progetto: colpisce anche la shell che lo
esegue. Sul sistema dell'autore può girare il suo Oxterm, che non va interrotto.

### Prova manuale con un `sshd` vero

Eseguita il 2026-11-01 con `sshd -f cfg` non privilegiato su `127.0.0.1:22222`,
autenticazione a chiave, `HOME` temporanea e `extra_args: ["-F", cfg]`.

| Verifica | Esito |
| --- | --- |
| Sessione aperta con `oxterm connect`, comando eseguito sul server | Ok |
| Secondo tab sulla stessa connessione condivisa, un solo login in `sshd` | Ok |
| Tunnel `-L` in stato `listening`, `curl` attraverso il tunnel | Ok |
| Stop del tunnel: processo terminato, porta chiusa | Ok |
| Tunnel su porta occupata: errore esplicito | Ok |
| Host sconosciuto con `BatchMode`: `Host key verification failed` visibile | Ok |
| `kill -9` di Oxterm: `ssh -N` termina per `PDEATHSIG` | Ok |
| Domanda sulla chiave host attraverso `oxterm-askpass` in una PTY | Ok: arriva alla persona |
| Ripristino dopo `kill -9`: chiede prima di riconnettere | Ok |
| Pannello, ricerca e form su Xvnc | Ok, controllati con screenshot |

### Prova sul server reale `debminis`

Eseguita il 2026-11-01 con `ssh -p 2223 andres@debminis`, su richiesta dell'autore,
senza modificare nulla sul server. Autenticazione a chiave, OpenSSH 10.3 sul server.

| Verifica | Esito |
| --- | --- |
| `oxterm connect`: tab con nome fisso, comando eseguito sul server | Ok |
| Secondo tab sulla connessione condivisa | Ok |
| Tunnel `-L` verso la porta SSH remota: `listening`, banner `SSH-2.0-OpenSSH_10.3p1` letto attraverso il tunnel | Ok |
| Stop del tunnel | Ok |
| Pannello: conteggio di 2 tab e avviso sul nome `debminis-live` | Ok |
| `sftp -P 2223`: comando `pwd` | Ok |
| Prompt reale con `PreferredAuthentications=password`: `andres@debminis's password: ` | Combacia con `decide` per la destinazione di `ssh -G` |

Per catturare il prompt, un finto askpass ha registrato il testo e terminato `ssh`
prima di rispondere, quindi non è stata inviata nessuna password. Con
`keyboard-interactive` il server non ha mostrato prompt. Il formato
`(user@host) Password:` resta quindi non provato dal vivo.

### Login reale con password

`scripts/e2e/password-login.sh` usa `ssh` reale, `oxterm-askpass` reale e un portachiavi
gnome-keyring reale in una sessione D-Bus privata. Finto è solo il server
(`paramiko`), perché un `sshd` senza root non verifica le password. Richiede
`python-paramiko`, `gnome-keyring` e `secret-tool`, quindi si lancia a mano e non in CI.

| Scenario | Esito |
| --- | --- |
| Password giusta, metodo `password` | Login, un solo tentativo |
| Password giusta, `keyboard-interactive` con `(user@host) Password:` | Login, un solo tentativo |
| Password sbagliata, entrambi i metodi | Inviata una sola volta, poi chiede alla persona |
| Nessuna password salvata | Chiede alla persona, nulla inviato |

### Pannello e form

`scripts/smoke-connections.sh` li guida con `xdotool`, con `ssh` e `secret-tool` finti.
Gira in CI nel job `rust`. Copre creazione, rifiuto di un host con `-`, ricerca e
connessione, password solo nel portachiavi, palette, `/help` e `/ssh` al prompt,
cancellazione con pulizia del portachiavi, ripristino con conferma e ordine.

Controllati a mano con screenshot: colori per connessione, lato destro dello split dopo
il ripristino, spostamento di un tab tra due processi.

## Hook e installazione

Il `post-commit` del repository reinstalla il binario in `PATH`. Durante questo
lavoro è stato escluso, per non mettere una build non ancora unita nel terminale
dell'autore. Il `pre-commit` con la build release è rimasto attivo. Dopo il merge,
il `post-merge` installa la nuova versione, compreso `oxterm-askpass` accanto a
`oxterm` (`scripts/refresh-installed.sh`).

## Registro

| Data | Lavoro |
| --- | --- |
| 2026-11-01 | Completati colori, palette, `/ssh`, conteggi, spostamento tra processi e ripristino. Corretti i comandi `/...` al prompt. Aggiunti `smoke-connections.sh` e `e2e/password-login.sh`. Unione in `master`. |
| 2026-11-01 | Fasi 0-5, tranne i colori per connessione. Correzione dei test `remote_attach` e del controllo AUR. Prova su `debminis`. [PR #2](https://github.com/buzzqw/oxterm/pull/2). |
