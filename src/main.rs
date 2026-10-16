//! Tela do Boteco: o app de transmissÃ£o.
//!
//! Em construÃ§Ã£o, e na ordem dos riscos: captura primeiro, codificaÃ§Ã£o depois,
//! rede por Ãºltimo, interface no fim. ComeÃ§ar pelo acabamento Ã© o jeito de
//! descobrir tarde que o caminho nÃ£o fecha.
//!
//! Uso:
//!   tela-boteco                  lista as janelas que dÃ£o para transmitir
//!   tela-boteco <n> [segundos]   captura a janela de nÃºmero n e mede
//!   tela-boteco <texto> [seg]    captura a primeira janela cujo tÃ­tulo casa

mod audio;
mod captura;
mod codificador;
mod janelas;
mod transmissao;
mod vivo;

fn main() {
    // Liga com RUST_LOG=webrtc_ice=debug para ver por que uma conexao nao fecha.
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        listar();
        return;
    }

    if args[0] == "--compartilhar" {
        // O PRODUTO: a tela indo ao vivo para quem assiste. Sem arquivo.
        let lista = janelas::listar();
        let alvo = args[1].to_lowercase();
        let Some(j) = lista.iter().find(|j| {
            j.titulo.to_lowercase().contains(&alvo) || j.executavel.to_lowercase().contains(&alvo)
        }) else {
            eprintln!("nao achei essa janela");
            std::process::exit(1);
        };
        let pasta = std::path::PathBuf::from(&args[2]);
        let segundos: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20.0);
        println!("compartilhando '{}' ao vivo...
", j.titulo);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        if let Err(e) = rt.block_on(transmissao::transmitir_janela(
            j.hwnd, j.pid, &pasta, segundos, 60, 8_000_000,
        )) {
            eprintln!("falhou: {e}");
            std::process::exit(1);
        }
        return;
    }

    if args[0] == "--transmitir" {
        // Teste da perna de rede: manda uma amostra de H.264 ja pronta para
        // um navegador. Separado da captura de proposito: provar que o video
        // CHEGA e um risco, e falar com o site da Vercel e outro. Misturar os
        // dois faria uma falha parecer a outra.
        let arquivo = std::path::PathBuf::from(&args[1]);
        let pasta = std::path::PathBuf::from(&args[2]);
        let fps: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(30);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        if let Err(e) = rt.block_on(transmissao::transmitir_arquivo(&arquivo, &pasta, fps)) {
            eprintln!("falhou: {e}");
            std::process::exit(1);
        }
        return;
    }

    if args[0] == "--vivo" {
        ao_vivo(&args[1..]);
        return;
    }

    if args[0] == "--audio" {
        ouvir(&args[1..]);
        return;
    }

    let segundos: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let lista = janelas::listar();

    // Por nÃºmero da lista, ou por pedaÃ§o do tÃ­tulo. NÃƒO por pid: um navegador
    // tem uma dezena de janelas no mesmo processo, e foi assim que a primeira
    // versÃ£o disto escolheu a janela errada. A identidade da janela Ã© o HWND;
    // o pid serve para o Ã¡udio, que Ã© outra pergunta.
    let escolhida = match args[0].parse::<usize>() {
        Ok(n) if n >= 1 && n <= lista.len() => lista.get(n - 1),
        _ => {
            // Casa pelo tÃ­tulo OU pelo executÃ¡vel: quem procura pensa no
            // aplicativo, e o tÃ­tulo costuma ser outra coisa. A janela do
            // Spotify se chama pelo nome da mÃºsica tocando.
            let alvo = args[0].to_lowercase();
            lista.iter().find(|j| {
                j.titulo.to_lowercase().contains(&alvo)
                    || j.executavel.to_lowercase().contains(&alvo)
            })
        }
    };

    let Some(j) = escolhida else {
        eprintln!("nao achei essa janela (roda sem argumento para ver a lista)");
        std::process::exit(1);
    };

    if j.minimizada {
        eprintln!("a janela esta minimizada: restaura ela antes, senao nao ha o que capturar");
        std::process::exit(1);
    }

    // Terceiro argumento liga a gravaÃ§Ã£o. Sem ele, sÃ³ mede a captura.
    let arquivo = args.get(2).cloned();
    let bitrate: u32 = args
        .get(3)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(12_000_000);

    println!("capturando '{}' ({}) por {segundos}s...\n", j.titulo, j.executavel);

    // O som vem do processo DONO DA JANELA escolhida. Escolher o que
    // transmitir e escolher a fonte de som sao a mesma acao, como no Discord.
    let pid_audio = if arquivo.is_some() { Some(j.pid) } else { None };

    match captura::capturar_janela(j.hwnd, segundos, arquivo.as_deref(), bitrate, pid_audio) {
        Ok(r) => {
            println!("tamanho   {}x{}", r.largura, r.altura);
            println!("quadros   {} em {:.1}s  =>  {:.1} por segundo", r.quadros, r.segundos, r.fps());
            println!("brilho    {:.1} de 255", r.brilho);

            if let Some(g) = &r.gravacao {
                println!();
                println!("gravado   {} ({} quadros)", g.arquivo, g.quadros);
                println!(
                    "som       {:.1}s de audio do processo {}",
                    g.amostras_audio as f64 / audio::TAXA as f64,
                    j.executavel
                );
                println!("encoder   {}", g.codificador);
                // TrÃªs respostas, e nÃ£o duas: "nÃ£o sei" Ã© uma delas. A versÃ£o
                // anterior afirmava "software" quando a pergunta falhava, e
                // mensagem errada custa mais caro que mensagem ausente: estava
                // acusando a CPU enquanto o chip da placa fazia o trabalho.
                println!(
                    "onde      {}",
                    match (g.em_hardware, g.codificador.as_str()) {
                        (true, _) => "CHIP DA PLACA (Ã© o que se quer)",
                        (false, "nenhum") =>
                            "nÃ£o consegui identificar (mede o tempo de CPU para saber)",
                        _ => "CPU em software (o Windows trocou sem avisar)",
                    }
                );
            }
            println!();
            // Captura quebrada costuma entregar quadro preto sem erro nenhum,
            // e um contador feliz esconderia isso.
            if r.quadros == 0 {
                println!("NENHUM quadro chegou: a captura nao esta funcionando.");
            } else if r.brilho < 1.0 {
                println!("Os quadros chegaram PRETOS: a captura abriu mas nao pegou a imagem.");
            } else {
                println!("Captura funcionando: os quadros chegaram com imagem de verdade.");
            }
        }
        Err(e) => {
            eprintln!("falhou: {e}");
            std::process::exit(1);
        }
    }
}

/// Mede o som de um processo. Sem ouvido: o que sai Ã© nÃ­vel, e nÃ­vel responde
/// a pergunta que importa.
///
/// O teste do isolamento Ã© feito com DOIS processos: um tocando e um mudo. Se
/// o mudo vier mudo com o outro tocando ao lado, a separaÃ§Ã£o Ã© real. Essa Ã© a
/// prova que nÃ£o depende de alguÃ©m escutar e achar que ouviu.
fn ouvir(args: &[String]) {
    let Ok(pid) = args.first().map(|s| s.parse::<u32>()).unwrap_or(Err("".parse::<u32>().unwrap_err())) else {
        eprintln!("uso: tela-boteco --audio <pid> [segundos] [excluir]");
        std::process::exit(2);
    };
    let segundos: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let modo = if args.get(2).map(|s| s == "excluir").unwrap_or(false) {
        audio::Modo::Excluir
    } else {
        audio::Modo::Incluir
    };

    println!(
        "ouvindo o pid {pid} por {segundos}s ({})...\n",
        if modo == audio::Modo::Incluir { "sÃ³ ele" } else { "tudo MENOS ele" }
    );

    let captura = match audio::CapturaAudio::iniciar(pid, modo) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("falhou: {e}");
            std::process::exit(1);
        }
    };

    let fim = std::time::Instant::now() + std::time::Duration::from_secs_f64(segundos);
    let (mut pedacos, mut amostras, mut pico, mut soma) = (0u64, 0u64, 0f32, 0f64);

    while std::time::Instant::now() < fim {
        let resta = fim - std::time::Instant::now();
        match captura.receptor.recv_timeout(resta.min(std::time::Duration::from_millis(200))) {
            Ok(p) => {
                pedacos += 1;
                amostras += p.amostras.len() as u64;
                for a in &p.amostras {
                    let v = a.abs();
                    if v > pico {
                        pico = v;
                    }
                    soma += (*a as f64) * (*a as f64);
                }
            }
            Err(_) => continue,
        }
    }
    captura.parar();

    let rms = if amostras > 0 { (soma / amostras as f64).sqrt() } else { 0.0 };
    let db = |v: f64| if v > 0.0 { 20.0 * v.log10() } else { -120.0 };

    println!("pedaÃ§os   {pedacos}");
    println!("amostras  {amostras}  ({:.1}s de som)", amostras as f64 / (audio::TAXA * audio::CANAIS as u32) as f64);
    println!("pico      {:.1} dB", db(pico as f64));
    println!("mÃ©dia     {:.1} dB", db(rms));
    println!();

    // -60 dB Ã© silÃªncio de verdade: ruÃ­do de fundo digital fica bem abaixo
    // disso, e som audÃ­vel fica bem acima.
    if amostras == 0 {
        println!("NADA chegou: a captura nÃ£o abriu ou o processo nÃ£o existe.");
    } else if db(rms) < -60.0 {
        println!("SILÃŠNCIO: este processo nÃ£o estava emitindo som.");
    } else {
        println!("TEM SOM: a captura pegou o Ã¡udio deste processo.");
    }
}

/// Captura ao vivo e codifica em memória, sem arquivo no caminho.
///
/// Opcionalmente despeja os pacotes num arquivo, mas isso é conferência: serve
/// para olhar o fluxo com outra ferramenta e confirmar que saiu H.264 de
/// verdade. O produto é a tela indo para quem assiste.
fn ao_vivo(args: &[String]) {
    let lista = janelas::listar();
    let alvo = args.first().map(|s| s.to_lowercase()).unwrap_or_default();
    let Some(j) = lista
        .iter()
        .find(|j| j.titulo.to_lowercase().contains(&alvo) || j.executavel.to_lowercase().contains(&alvo))
    else {
        eprintln!("nao achei essa janela");
        std::process::exit(1);
    };
    let segundos: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let despejo = args.get(2).cloned();

    println!("capturando '{}' ao vivo por {segundos}s...\n", j.titulo);

    let (envio, recebe) = std::sync::mpsc::channel::<Vec<u8>>();
    let coletor = std::thread::spawn(move || {
        let mut pacotes = 0u64;
        let mut bytes = 0u64;
        let mut arquivo = despejo.as_ref().map(|c| std::fs::File::create(c).expect("arquivo"));
        let mut primeiro: Vec<u8> = Vec::new();
        while let Ok(p) = recebe.recv() {
            if primeiro.is_empty() {
                primeiro = p.iter().take(8).copied().collect();
            }
            pacotes += 1;
            bytes += p.len() as u64;
            if let Some(f) = arquivo.as_mut() {
                use std::io::Write;
                let _ = f.write_all(&p);
            }
        }
        (pacotes, bytes, primeiro)
    });

    match captura::capturar_ao_vivo(j.hwnd, segundos, 60, 8_000_000, envio) {
        Ok((quadros, nome, hardware)) => {
            let (pacotes, bytes, primeiro) = coletor.join().unwrap_or((0, 0, Vec::new()));
            println!("quadros capturados  {quadros}");
            println!("pacotes de H.264    {pacotes}  ({:.2} MB)", bytes as f64 / 1_048_576.0);
            println!("codificador         {nome}");
            println!("onde                {}", if hardware { "CHIP DA PLACA" } else { "CPU em software" });
            // Os quatro primeiros bytes dizem o formato: 00 00 00 01 é Annex-B,
            // que é o que o WebRTC espera receber. Qualquer outra coisa teria de
            // ser convertida antes de ir para a rede.
            println!(
                "começo do fluxo     {}",
                primeiro.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
            );
            if pacotes == 0 {
                println!("\nNENHUM pacote saiu: o codificador não produziu nada.");
            } else {
                println!("\nCodificação ao vivo funcionando, sem arquivo no caminho.");
            }
        }
        Err(e) => {
            eprintln!("falhou: {e}");
            std::process::exit(1);
        }
    }
}

fn listar() {
    let lista = janelas::listar();
    println!("{} janela(s) que dariam para transmitir:\n", lista.len());
    println!("{:>3}  {:<10} {:<22} {:>11}   {}", "n", "pid", "executÃ¡vel", "tamanho", "tÃ­tulo");
    println!("{}", "-".repeat(98));

    for (i, j) in lista.iter().enumerate() {
        let titulo: String = j.titulo.chars().take(44).collect();
        let tamanho = if j.minimizada {
            "minimizada".to_string()
        } else {
            format!("{}x{}", j.largura, j.altura)
        };
        println!("{:>3}  {:<10} {:<22} {:>11}   {}", i + 1, j.pid, j.executavel, tamanho, titulo);
    }

    if lista.is_empty() {
        println!("(nenhuma: isso Ã© suspeito, e vale olhar os filtros em janelas.rs)");
    } else {
        println!("\nPara capturar uma: tela-boteco <n> [segundos]");
    }
}


