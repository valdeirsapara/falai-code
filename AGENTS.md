# falai-code

Uma aplicação que permite usar agentes de código pelo terminal em conversa
natural: você fala o que quer, o agente trabalha e a resposta volta em voz alta,
como numa conversa com uma pessoa. Não é preciso digitar comandos nem ler
blocos de código para acompanhar o que está acontecendo.

Este arquivo é o guia para agentes de código (e pessoas) que trabalham neste
repositório.

## Objetivo

Um binário em Rust, `voice`, que envolve qualquer agente de código de terminal
(Claude Code, OpenCode, Codex) e acrescenta conversa por voz nos dois sentidos,
sem MCP e sem alterar o agente.

```
voice claude
voice opencode
voice codex
voice ai-memory claude   # encadeado com outros wrappers
```

O fluxo pretendido:

1. Abrir o agente dentro de um PTY, com a interface original visível.
2. Gravar o microfone enquanto uma tecla está pressionada (push-to-talk).
3. Transcrever a fala e injetar o texto no agente, com opção de revisar antes.
4. Ler em voz alta só o texto final da resposta, sem código, diffs ou saídas de
   ferramentas, com um resumo do tipo "alterei 3 arquivos".
5. Parar a fala quando o usuário apertar a tecla de novo (barge-in).

Prioridades: Linux e WSL2 com WSLg primeiro, macOS depois. Português do Brasil
na transcrição e na voz. Roda no CPU (alvo: Intel Core Ultra 7 258V, sem GPU
dedicada). Nenhuma chave nem áudio salvos em disco por padrão.

Fora do MVP: interface gráfica, detecção de fala sem tecla, Windows nativo.

## Estado atual

Existe só o primeiro teste de áudio: `voice mic-test`.

```
cargo run -q -- mic-test                # push-to-talk: segure ESPAÇO, solte para ouvir
cargo run -q -- mic-test --key v        # outra tecla
cargo run -q -- mic-test --seconds 2    # grava 2 s sem tecla (não precisa de TTY)
cargo run -q -- mic-test --no-play      # não toca de volta
```

Ele grava em 16 kHz mono, mostra um medidor de volume, informa pico e média em
dBFS, mede a latência e toca a gravação de volta. Foi validado no WSL2 com WSLg
em 17/10/2026, inclusive o push-to-talk em terminal interativo.

### Estrutura

| Arquivo | Papel |
| --- | --- |
| `src/main.rs` | CLI (`clap`), modo raw do terminal, loop do `mic-test` |
| `src/audio.rs` | Captura contínua do microfone em uma thread, reprodução, estatísticas |
| `src/ptt.rs` | Detecção de segurar e soltar a tecla, com testes unitários |

## Decisões e descobertas

Leia antes de mudar a base de áudio ou de teclado.

- **Áudio via PulseAudio, não `cpal`.** O WSLg expõe um servidor PulseAudio em
  `unix:/mnt/wslg/PulseServer`, e o Linux nativo oferece a mesma API pelo
  PipeWire. O `cpal` usa ALSA, que no WSL exige a ponte `alsa-plugins`. Usamos
  `libpulse-simple-binding`. Para o macOS, o plano é pôr o `cpal` atrás da mesma
  interface.
- **O microfone do WSLg chega em blocos.** A fonte é `RDPSource` (o áudio vem do
  Windows pelo protocolo RDP) e entrega dados em rajadas de até ~200 ms. Ao
  soltar a tecla, `Capture::end` espera o áudio que ainda está a caminho, até
  600 ms, para não cortar a última palavra. Medido: 10 a 30 ms em geral, 189 ms
  no pior caso. Esse tempo conta no orçamento de latência.
- **Terminais não informam quando a tecla é solta.** Sem o protocolo de teclado
  do kitty, a soltura é detectada quando a repetição automática para (700 ms
  antes da primeira repetição, 150 ms entre repetições). Com o protocolo, o
  evento de soltura é usado direto. Atalhos globais não funcionam no WSL porque o
  Linux não recebe as teclas do Windows.
- **Capturar a resposta pelos arquivos de sessão, não pela tela.** Ler a saída
  do PTY é frágil por causa dos códigos de controle da interface, e o modo
  headless (`claude -p --output-format stream-json`) é outra sessão, sem a
  interface interativa. O plano é acompanhar o que cada agente já grava em disco:
  Claude Code em `~/.claude/projects/<projeto>/*.jsonl`, Codex em
  `~/.codex/sessions/`, OpenCode em `~/.local/share/opencode/`. Esses arquivos já
  separam o texto final das chamadas de ferramenta. No encadeamento
  (`voice ai-memory claude`), o agente é identificado procurando o nome nos
  argumentos.
- **Meta de latência redefinida.** "Menos de 1,5 s entre soltar a tecla e ouvir
  a resposta" é impossível, porque o modelo pode levar minutos. A meta passa a
  ser menos de 1,5 s entre o agente terminar e a voz começar, mais um aviso curto
  logo depois de soltar a tecla.
- **Binário único estático está em aberto.** `whisper-rs` traz C++, o Kokoro
  precisa do ONNX Runtime e a captura depende de bibliotecas do sistema. A
  proposta é um binário glibc com cada provedor como feature opcional.
- **A NPU não é visível no WSL2.** Não existe `/dev/accel`; o WSL só repassa a
  GPU (`/dev/dxg`). Ver microsoft/WSL#40842. O caminho viável é um provedor que
  fala com um servidor OpenVINO no Windows por `localhost`. O Whisper do OpenVINO
  GenAI aceita NPU; o Kokoro na NPU não foi confirmado. Fica para depois do MVP.

## Provedores planejados

- Transcrição: Groq (nuvem) e whisper.cpp local.
- Voz: Piper, Kokoro, edge-tts e Picovoice Orca (confirmar se o Orca tem pt-BR).
- Configuração em `~/.config/voice-harness/config.toml`; o comando `voice doctor`
  vai testar microfone, áudio e provedores.

## Próximos passos

1. Ler o JSONL de sessão do Claude Code e falar a última resposta (Piper ou
   edge-tts).
2. Mandar a gravação para o Groq ou o whisper.cpp e conferir a transcrição em
   português.
3. Wrapper de PTY (`portable-pty`) com push-to-talk e injeção do texto.
4. Suporte a Codex e OpenCode.
5. `voice doctor`.

## Convenções

- Código, nomes e comentários em inglês; mensagens da CLI em português do Brasil.
- Áudio só na memória. Não gravar áudio nem chaves em disco sem opção explícita.
- Antes de commitar: `cargo fmt`, `cargo clippy` sem avisos e `cargo test`.
- Requisitos de sistema para compilar no Arch: `libpulse` (já vem com o
  PulseAudio/PipeWire).
