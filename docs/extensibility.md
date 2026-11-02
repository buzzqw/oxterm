# Estensioni e plugin: progetto

Documento di lavoro per l'estensibilità di Oxterm. Descrive il modello scelto,
la specifica della Fase 1 e le fasi successive. Non è ancora codice: serve a
fissare formato, protocollo, confini e test prima di toccare il terminale.

Ultimo aggiornamento: 2026-11-02.

## Principi

1. **La logica senza display sta in `src/headless/`.** Parsing, validazione,
   matching dei trigger e serializzazione degli eventi non usano GTK/VTE e sono
   coperti da test normali. Solo il collegamento ai widget resta in `src/`.
2. **Nessuna dipendenza nuova per la Fase 1.** Trigger, eventi e azioni usano
   `serde_json`, il control socket e i moduli già presenti.
3. **Tre livelli, introdotti nell'ordine.** Dichiarativo (JSON), processi
   esterni sul control socket, WASM opzionale. Ogni livello deve essere utile da
   solo.
4. **Il binario resta piccolo.** Niente runtime sempre linkati: WASM vivrà
   dietro una feature Cargo o in un helper separato, coerente con LTO, `strip`,
   `codegen-units = 1` e `panic = "abort"`.
5. **La sicurezza è parte del formato.** Un trigger che scrive input o esegue
   comandi è disabilitato per default e richiede un consenso esplicito. Gli
   eventi non contengono contenuto dello schermo se non richiesto.
6. **Il broker è un vantaggio, non un dettaglio.** Un plugin esterno deve poter
   girare sull'host remoto e parlare con Oxterm attraverso il broker, come già
   fanno i comandi di controllo.

## Fase 1 — Trigger, eventi, azioni

Tutti e tre i pezzi sono necessari: i trigger da soli sanno fare solo azioni
fisse, gli eventi da soli non possono reagire all'output, le azioni da sole non
hanno chi le invoca.

### 1.1 File dei trigger

File dedicato, non una chiave di `settings.json`: i trigger sono una lista di
oggetti, non un valore scalare, e `src/headless/spec.rs` descrive solo valori
scalari. Il file vive accanto alla configurazione:

```text
~/.config/oxterm/triggers.json          (o $XDG_CONFIG_HOME/oxterm/)
```

```json
{
  "version": 1,
  "triggers": [
    {
      "id": "compiler-errors",
      "enabled": true,
      "match": "^([^:/]+):(\\d+):(\\d+): (error|warning): (.*)$",
      "actions": [
        { "type": "highlight_line", "fg": "#ff5555" },
        {
          "type": "capture",
          "list": "errors",
          "label": "{1}:{2}:{3}  {5}",
          "open": "editor_jump"
        }
      ]
    },
    {
      "id": "test-failures",
      "match": "^test .* \\.\\.\\. FAILED$",
      "actions": [
        { "type": "notify", "title": "Test fallito", "body": "{0}" },
        { "type": "set_tab_color", "color": "#cc3333" }
      ]
    }
  ]
}
```

Campi:

| Campo | Tipo | Note |
| --- | --- | --- |
| `version` | int | Obbligatorio, oggi `1`. Un file con versione maggiore si apre in sola lettura. |
| `id` | string | Unico, usato nei log e per disabilitare un trigger. |
| `enabled` | bool | Default `true`. |
| `match` | string | Regex Rust (`regex` crate). Obbligatoria. |
| `actions` | array | Almeno una azione. |
| `profiles` | array | Facoltativo: applica il trigger solo ai profili elencati. |
| `scope` | string | `line` (default) o `command_output` (solo tra OSC 133 prompt e fine comando). |

Azioni della v1, tutte senza esecuzione di codice:

| Tipo | Effetto | Parametri |
| --- | --- | --- |
| `highlight_line` | Colora l'intera riga | `fg`, `bg` |
| `highlight_text` | Colora solo il match | `fg`, `bg` |
| `notify` | Notifica desktop | `title`, `body` |
| `capture` | Aggiunge a una lista "Captured output" | `list`, `label`, `open` |
| `set_tab_color` | Colora la tab | `color` |
| `set_tab_title` | Cambia il titolo della tab | `title` |
| `hyperlink` | Rende il match cliccabile | `url` |
| `mark_failure` | Segna la tab come fallita | nessuno |

Nei parametri `{0}` è il match completo, `{1}`..`{9}` i gruppi; `{{` e `}}`
sono parentesi letterali.

Azioni rinviate a una versione successiva, perché eseguono codice o scrivono
input: `inject` (invia byte al PTY) e `run` (esegue un comando). Se arriveranno,
richiederanno `"allow_exec": true` nel file e un avviso visibile.

#### Come leggere l'output con VTE 0.70

VTE non espone un callback per riga. Il punto di aggancio è
`contents_changed`, con queste regole:

1. Un `TriggerScanner` in `src/headless/triggers.rs` conserva un cursore
   monotono `(row, column)` nell'area di testo.
2. Al segnale, il lato GTK chiama `vte_terminal_get_text_range` dal cursore alla
   posizione corrente, divide in righe complete e passa solo quelle al modulo
   headless.
3. Gli eventi sono accorpati con un timer breve (50 ms) per non scansionare a
   ogni byte.
4. Se il cursore torna indietro (scrollback tagliato, reset, clear), lo scanner
   si riallinea alla prima riga visibile e riparte.
5. Il matching avviene su testo semplice, senza attributi. Il colore lo applica
   il lato GTK con le API di selezione/attributi disponibili; per la v1 è
   accettabile colorare la riga tramite un overlay o il margine, se VTE non
   permette di modificare attributi già stampati.

Il punto 5 è il rischio tecnico principale: VTE non consente di riscrivere gli
attributi del testo già nel buffer. Prima di implementare `highlight_*` va
verificato se basta un overlay disegnato sul widget; in alternativa la v1
espone solo `capture`, `notify`, `set_tab_color`, `set_tab_title`, `mark_failure`
e rimanda l'evidenziazione.

### 1.2 Eventi

Il control socket già usa frame JSON con prefisso di lunghezza
(`src/headless/framing.rs`). Si aggiunge una connessione long-lived:

```text
→ {"op":"subscribe","events":["command_finished","bell"],"include_output":false}
← {"ok":true,"subscription":1}
← {"event":"command_finished","pane":"12345-2","tab":"12345-1","exit":1,
   "duration_ms":4213,"cwd":"/src/app","time":"2026-11-02T09:12:33Z"}
← {"event":"bell","pane":"12345-2","time":"..."}
```

Eventi della v1:

| Evento | Payload |
| --- | --- |
| `command_started` | `pane`, `command`, `cwd`, `time` |
| `command_finished` | `pane`, `exit`, `duration_ms`, `cwd`, `time` |
| `prompt` | `pane`, `cwd`, `time` |
| `bell` | `pane`, `time` |
| `title_changed` | `pane`, `title`, `time` |
| `cwd_changed` | `pane`, `cwd`, `time` |
| `tab_created` / `tab_closed` | `tab`, `pane`, `time` |
| `pane_focused` | `pane`, `tab`, `time` |
| `selection_changed` | `pane`, `length`, `time` (mai il testo) |
| `theme_changed` | `light`, `time` |

`output_line` è previsto ma **opt-in** per sottoscrizione
(`"include_output":true`) e con un limite di righe al secondo, perché può
contenere segreti. Default: spento.

Implementazione senza GTK per la parte testabile:

```rust
pub struct EventHub {
    subscribers: Vec<EventSubscriber>, // mpsc::Sender<Value> + filtro
}

impl EventHub {
    pub fn publish(&self, event: &Event);
    pub fn subscribe(&mut self, filter: EventFilter) -> SubscriptionId;
}
```

Il lato GUI possiede un solo `EventHub`; i punti che già conoscono OSC 133, il
bell, il titolo e la directory pubblicano qui. Le code sono limitate: un
sottoscrittore lento viene disconnesso, non blocca la GUI.

CLI:

```text
oxterm events [--pane ID] [--event NAME]... [--output] [--json]
```

Stampa un oggetto JSON per riga e termina quando il socket si chiude. Senza
`--json` stampa una forma leggibile, utile a mano.

### 1.3 Azioni

Nuova operazione sul socket:

```text
→ {"op":"action","action":"set_tab_color","pane":"12345-2","color":"#cc3333"}
← {"ok":true}
```

Azioni della v1: `set-tab-title`, `set-tab-color`, `notify`, `open-url`,
`open-path`, `insert-text`, `send`, `new-tab`, `split`, `set-status`.

CLI corrispondente:

```text
oxterm action set-tab-color --pane ID COLOR
oxterm action set-tab-title --pane ID TITLE
oxterm action notify --title T --body B
oxterm action open-url URL
oxterm action set-status --pane ID TEXT
```

`send` e `insert-text` riusano l'implementazione di `oxterm send`; `new-tab` e
`split` riusano le operazioni esistenti. Nessuna azione accetta comandi di shell
arbitrari: per quello c'è già `-- COMMAND` in `new`/`split`.

### 1.4 File e responsabilità

| File | Ruolo | GTK |
| --- | --- | --- |
| `src/headless/triggers.rs` | Parsing, validazione, matching, effetti, test | No |
| `src/headless/events.rs` | Schema eventi, filtri, `EventHub` | No |
| `src/headless/control_cli.rs` | Parsing di `events` e `action` | No |
| `src/control.rs` | Client e server delle nuove operazioni | Sì |
| `src/terminal.rs` | `contents_changed` → scanner → effetti | Sì |
| `src/window.rs` | Colore/titolo tab, notifiche, liste catturate | Sì |
| `manual.md` | Documentazione utente | — |

`docs/settings.md` non cambia: i trigger non sono impostazioni scalari. Il
manuale spiega il file e i comandi.

### 1.5 Sicurezza

- Il socket resta privato dell'utente, con controllo `SO_PEERCRED` come oggi.
- Al massimo 8 sottoscrizioni contemporanee, code limitate, disconnessione dei
  client lenti.
- `output_line` è spento per default e limitato per frequenza.
- Nessun trigger può leggere o modificare la riga di input corrente, per non
  intercettare password.
- `inject` e `run` non esistono nella v1. Se arriveranno, solo con consenso
  esplicito e indicatore visibile nella tab.
- Le notifiche non includono mai il testo dello schermo se non richiesto
  esplicitamente dal trigger.

### 1.6 Piano di test

Test headless (`src/headless/triggers.rs`, `events.rs`):

- parsing di un file valido, versioni future, campi sconosciuti, regex invalida;
- `{0}`/`{n}` con gruppi mancanti, `{{`/`}}`;
- ordine delle azioni e stop al primo match se `"once": true` (campo previsto);
- serializzazione degli eventi e filtri per pane/evento;
- `EventHub`: sottoscrittore lento, limite, disconnessione.

Test del control API (`tests/control_api.rs`):

- `subscribe` riceve `command_started`/`command_finished` dopo un comando vero;
- `action set-tab-color` cambia lo stato e `--info` lo riporta;
- azione su pane inesistente → codice 3;
- nessuna risposta a peer non autorizzato (se il test lo consente).

Test manuali:

```bash
printf 'error: boom\n'          # trigger highlight/capture
oxterm events | jq .            # eventi in diretta
oxterm action set-tab-color --pane "$OXTERM_PANE" '#cc3333'
```

## Fase 2 — Widget, contributi, workspace

- **Status widget.** `~/.config/oxterm/widgets.json`: un comando, un intervallo,
  un renderer (`text`, `git`, `graph`), un'azione al click. Esecuzione in un
  processo figlio con timeout (2 s), cache dell'ultimo valore, nessun blocco
  della GUI.
- **Contributi.** Lo stesso file (o il manifest della Fase 3) dichiara voci per
  la command palette e per il menu contestuale: `id`, `label`, `command`,
  `keys`, `when`.
- **Workspace.** `oxterm workspace FILE` con JSON, non YAML: il progetto non ha
  una dipendenza YAML e non vale la pena aggiungerne una. Struttura: finestre →
  tab → pane, con `cwd`, `title`, `color`, `command`, `profile`. Riusa `new` e
  `split`.

## Fase 3 — Pacchetti plugin

- Directory: `$XDG_DATA_HOME/oxterm/plugins/<id>/`.
- Manifest `plugin.json`: `id`, `name`, `version`, `api_version`, `permissions`,
  `settings`, `entrypoints`.
- Comandi: `oxterm plugin install|list|update|remove`, lockfile
  `plugins.lock.json` con URL e commit.
- Permessi: `read-screen`, `send-input`, `spawn`, `open-files`, `network`,
  `clipboard`, `notify`, `workspace`, `status-bar`. Richiesta al primo uso,
  revoca dalle preferenze, indicatore nella tab quando un plugin è attivo.
- API del plugin = processo esterno che parla il protocollo `events`/`action`
  della Fase 1. Nessun runtime nuovo.

## Fase 4 — Pannelli WASM e MCP

- **WASM** con `wasmtime` dietro feature `plugins-wasm`, oppure in un helper
  `oxterm-plugin-host` separato per non gonfiare il binario principale.
  Capacità esplicite, niente accesso al filesystem se non concesso, rendering
  in un `DrawingArea` con una griglia di celle o un buffer ANSI.
- **MCP** come binario separato `oxterm-mcp` che espone `new`, `split`, `send`,
  `capture`, `wait` come tool MCP. Non richiede WASM e può arrivare prima della
  Fase 4: è il modo più economico per far pilotare Oxterm a un agente.

## Stato dell'implementazione (2026-11-02)

La Fase 1 è implementata in questo ramo:

| Pezzo | File | Stato |
| --- | --- | --- |
| Parsing, validazione e matching dei trigger | `src/headless/triggers.rs` | fatto, con test |
| Event bus e filtri | `src/headless/events.rs` | fatto, con test |
| CLI `events` / `action` / `capture --captured` | `src/headless/control_cli.rs` | fatto, con test |
| Client e server del control socket | `src/control.rs` | fatto |
| Trigger su `contents_changed` | `src/terminal.rs` | fatto |
| Eventi `command_started`, `command_finished`, `prompt`, `bell`, `title_changed`, `cwd_changed`, `tab_created`, `tab_closed`, `pane_focused`, `selection_changed` | `src/terminal.rs`, `src/window.rs` | fatto |
| Azioni `set-tab-color`, `set-tab-title`, `notify`, `open-url`, `insert-text`, `mark-failure`, `clear-failure` | `src/control.rs`, `src/window.rs` | fatto |
| `output_line` | — | previsto ma non ancora pubblicato |
| `theme_changed` | — | nome riservato, non ancora emesso |
| `highlight_line`, `highlight_text`, `hyperlink` | `src/headless/triggers.rs` | accettati e interpretati, non applicati: VTE 0.70 non consente di ristilare testo già stampato |

Deviazioni rispetto alla prima stesura:

- Il colore della tab è il colore del **testo** dell'etichetta, non lo sfondo:
  il markup di GTK è l'unico punto che non richiede CSS per tab.
- `capture` non ha un pannello dedicato: le righe finiscono in una lista per
  pane, letta con `oxterm capture --captured` e svuotata con `--clear`. Un
  pannello può arrivare nella Fase 2.
- I trigger si leggono una volta per processo. Il ricaricamento a caldo resta
  nella Fase 2, insieme ai widget.

## Decisioni aperte

1. ~~Trigger in `settings.json` o file dedicato?~~ File dedicato, implementato.
2. ~~Evidenziazione con VTE 0.70.~~ Rinviata: il testo già stampato non si può
   ristilare. Resta da valutare un overlay sul widget.
3. **Formato workspace.** JSON, per non aggiungere dipendenze.
4. **Runtime WASM.** `wasmtime` o `extism`; decidere solo dopo aver misurato
   l'impatto sul binario.
5. **Eventi come JSON Lines o frame?** Sul socket restano frame come oggi; la
   CLI stampa JSON Lines per comodità di `jq`.

## Registro

- 2026-11-02: prima stesura. Nessun codice scritto; Fase 1 pronta per
  l'implementazione.
- 2026-11-02: Fase 1 implementata (trigger, eventi, azioni, `capture
  --captured`) con test headless. Evidenziazione e hyperlink rinviati per i
  limiti di VTE 0.70.
