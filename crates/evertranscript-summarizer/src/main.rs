//! The local Summary engine, in its own process (ADR-0031).
//!
//! A small program: read a JSONL request from stdin, answer on stdout. All
//! of the interesting decisions live in the Core, which supervises this —
//! and that split is the point. **The Core must never die, because it is
//! the thing that is recording.** llama.cpp is a large C++ library doing
//! arithmetic on gigabytes of weights; a fault in it should cost a Summary,
//! not a meeting. ADR-0031 cites a competitor who embedded it in-process and
//! abandoned that path.
//!
//! Two consequences show up in the shape of this file:
//!
//! - **stdin EOF ends the process.** If the Core dies, its end of the pipe
//!   closes, `read_line` returns 0, and this exits. Without that a crashed
//!   Core leaves a multi-gigabyte model resident with nobody to stop it.
//! - **Nothing here is clever.** No caching beyond the loaded model, no
//!   retries, no policy. Everything that could be wrong in an interesting
//!   way belongs on the other side of the pipe where it can be tested
//!   without a model.

use std::io::BufRead;
use std::io::Write;
use std::num::NonZeroU32;

use evertranscript_core::summary::sidecar::SidecarRequest;
use evertranscript_core::summary::sidecar::SidecarResponse;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::AddBos;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;

/// Context window. Large enough for a chunk plus its prompt, small enough
/// that the KV cache does not evict the Operator's other work from memory —
/// this runs on a laptop that is also in a meeting.
const CONTEXT_TOKENS: u32 = 8_192;

/// Text that means the model has stopped summarizing and started inventing.
///
/// **Substring matching on the accumulated output** (catalog M4), not token
/// matching: a stop sequence can be split across tokens exactly the way a
/// CJK character can, and a token-level check would miss it. Small models
/// reliably continue past the answer — the first real run produced a correct
/// summary and then wrote itself a fresh transcript to summarize.
const STOP_SEQUENCES: &[&str] = &[
    "<transcript>",
    // **The closing tag, which is the one that actually shows up.** The
    // opening tag was listed and the closing one was not, and a model that
    // replays the prompt reaches `</transcript>` — so the guard was on the
    // marker that cannot appear and absent on the marker that does. Safe to
    // stop on because `prompt::escape_control_markers` puts a zero-width
    // space inside both tags in every untrusted string, so a transcript that
    // quotes one cannot produce a literal here: if this appears, the model
    // wrote it, and it has left the answer.
    "</transcript>",
    "\nSummary:",
    "\n\n\n\n",
    // Prompt scaffolding. A small model handed a long prompt will restate
    // it rather than answer it, and the first end-to-end run put this exact
    // sentence into a Meeting's Summary.
    "The operator's own notes",
];

/// How many recent tokens the repetition penalty considers.
const REPEAT_WINDOW: i32 = 256;

/// How hard repetition is discouraged. Enough to break a loop, not enough to
/// stop a summary using a name twice.
const REPEAT_PENALTY: f32 = 1.15;

/// Ceiling on a single answer. A summary that has not ended by here is a
/// model that has started repeating itself.
const MAX_OUTPUT_TOKENS: i32 = 2_048;

mod fit;

fn main() -> anyhow::Result<()> {
    // Before anything expensive. A Summary must never be the reason a
    // recording stutters, and asking the scheduler is cheaper than any
    // arrangement where the two processes have to know about each other.
    let lowered = fit::lower_priority();
    eprintln!(
        "priority: {}",
        if lowered { "lowered" } else { "unchanged" }
    );

    let backend = LlamaBackend::init()?;
    let mut loaded: Option<(String, LlamaModel)> = None;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        // An unreadable line is the Core going away mid-write, which is the
        // same situation as EOF: leave.
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<SidecarRequest>(line) {
            Ok(SidecarRequest::Load { model_path }) => match load(&backend, &model_path) {
                Ok((name, model)) => {
                    loaded = Some((name.clone(), model));
                    SidecarResponse::Ready { model: name }
                }
                Err(error) => SidecarResponse::Error {
                    message: error.to_string(),
                },
            },
            Ok(SidecarRequest::Generate { system, user }) => match loaded.as_ref() {
                Some((_, model)) => match generate(&backend, model, &system, &user) {
                    Ok(text) => SidecarResponse::Generated { text },
                    Err(error) => SidecarResponse::Error {
                        message: error.to_string(),
                    },
                },
                None => SidecarResponse::Error {
                    message: "no model loaded".into(),
                },
            },
            Ok(SidecarRequest::Ping) => SidecarResponse::Pong,
            Ok(SidecarRequest::Shutdown) => break,
            Err(error) => SidecarResponse::Error {
                message: format!("could not read the request: {error}"),
            },
        };

        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}

fn load(backend: &LlamaBackend, path: &str) -> anyhow::Result<(String, LlamaModel)> {
    // **Two loads, and the first one is cheap.** How many layers to offload
    // depends on how many the model has, and that is only knowable from the
    // model — but it has to be decided *before* the load it configures. A
    // vocab-only load reads the metadata without the weights, which answers
    // the question for the price of parsing a header.
    //
    // **Read from the metadata, not from `n_layer()`.** A vocab-only model
    // reports zero layers — measured, after the first version of this
    // silently offloaded nothing and turned a Metal load into a CPU one
    // while every test stayed green. The block count lives under the
    // architecture's own key, which is the general GGUF convention and so
    // works for whatever model is registered next.
    let layers = LlamaModel::load_from_file(
        backend,
        path,
        &LlamaModelParams::default().with_vocab_only(true),
    )
    .ok()
    .and_then(|metadata| {
        let architecture = metadata.meta_val_str("general.architecture").ok()?;
        metadata
            .meta_val_str(&format!("{architecture}.block_count"))
            .ok()?
            .parse::<u32>()
            .ok()
    })
    .unwrap_or(0);

    let model_bytes = std::fs::metadata(path).map(|file| file.len()).unwrap_or(0);
    let memory = fit::total_memory_bytes();
    let offload = fit::layers_that_fit(model_bytes, layers, memory);

    // Said out loud, because "it fits" is otherwise a claim nobody can check
    // — which is exactly how the criterion this replaces stayed false for a
    // milestone. stderr is inherited by the Core, so this lands in its log.
    eprintln!(
        "fit: {offload} of {layers} layers offloaded ({} MB of weights, {} MB of memory)",
        model_bytes / 1_048_576,
        memory / 1_048_576
    );

    let model = LlamaModel::load_from_file(
        backend,
        path,
        &LlamaModelParams::default().with_n_gpu_layers(offload),
    )
    .map_err(|error| {
        // A load that fails when nothing was offloaded is a model this
        // machine cannot hold at all, which is a different problem from a
        // missing file and deserves to say so.
        if offload == 0 {
            anyhow::anyhow!(
                "this model does not fit on this machine: {} MB of weights against \
                 {} MB of memory ({error})",
                model_bytes / 1_048_576,
                memory / 1_048_576
            )
        } else {
            anyhow::anyhow!("{error}")
        }
    })?;
    let name = std::path::Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "local".to_string());
    Ok((name, model))
}

fn generate(
    backend: &LlamaBackend,
    model: &LlamaModel,
    system: &str,
    user: &str,
) -> anyhow::Result<String> {
    let mut context = model.new_context(
        backend,
        LlamaContextParams::default().with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS)),
    )?;

    // A plain instruct framing rather than a model-specific chat template.
    // The Core has already escaped anything in `user` that could be read as
    // a turn boundary, so this cannot be steered by the transcript.
    let prompt = format!("{system}\n\n{user}\n\nSummary:\n");
    let tokens = model.str_to_token(&prompt, AddBos::Always)?;

    let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
    let last = tokens.len().saturating_sub(1);
    for (index, token) in tokens.iter().enumerate() {
        batch.add(*token, index as i32, &[0], index == last)?;
    }
    context.decode(&mut batch)?;

    // Greedy alone loops: a small model handed a transcript will happily
    // restate it forever, which the first real run did — the same four
    // lines, five times, until the token ceiling. A repetition penalty over
    // a window of recent tokens is what stops it, and greedy selection after
    // it keeps a summary reproducible for the same input, which matters for
    // a record somebody may re-generate and compare.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::penalties(REPEAT_WINDOW, REPEAT_PENALTY, 0.0, 0.0),
        LlamaSampler::greedy(),
    ]);
    let mut decoder = evertranscript_core::summary::sidecar::IncrementalUtf8::new();
    let mut out = String::new();

    for produced in 0..MAX_OUTPUT_TOKENS {
        // The prompt occupied 0..tokens.len()-1, so the first generated
        // token goes at exactly tokens.len(). Positions must be contiguous:
        // llama.cpp rejects a batch with a gap, and reports it as a bare
        // -1 that this crate's error text calls "n_tokens == 0" — which is
        // not what happened and cost an hour to see past.
        let position = tokens.len() as i32 + produced;
        let token = sampler.sample(&context, -1);
        sampler.accept(token);
        if model.is_eog_token(token) {
            break;
        }

        // **Bytes, then incremental decode.** A token is not a character:
        // one Chinese character is three bytes and a tokenizer will split
        // them across two tokens. Decoding each token independently turns
        // 会 into replacement characters, permanently, in a record that is
        // immutable by design. This product has already paid for Chinese
        // handling once.
        // 64 bytes is generous for one piece; the longest single token in
        // these vocabularies is well under it, and a piece that did not fit
        // would error rather than truncate.
        out.push_str(&decoder.push(&model.token_to_piece_bytes(token, 64, false, None)?));

        if let Some(cut) = STOP_SEQUENCES
            .iter()
            .filter_map(|stop| out.find(stop))
            .min()
        {
            out.truncate(cut);
            break;
        }

        batch.clear();
        batch.add(token, position, &[0], true)?;
        context.decode(&mut batch)?;
    }
    out.push_str(&decoder.finish());
    Ok(out)
}
