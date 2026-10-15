//! Tela do Boteco: o app de transmissão.
//!
//! Em construção, e na ordem dos riscos: captura primeiro, codificação depois,
//! rede por último, interface no fim. Começar pelo acabamento é o jeito de
//! descobrir tarde que o caminho não fecha.
//!
//! Uso:
//!   tela-boteco                  lista as janelas que dão para transmitir
//!   tela-boteco <n> [segundos]   captura a janela de número n e mede
//!   tela-boteco <texto> [seg]    captura a primeira janela cujo título casa

mod audio;
mod captura;
mod codificador;
mod janelas;
mod transmissao;

fn main() {
    // Liga com RUST_LOG=webrtc_ice=debug para ver por que uma conexao nao fecha.
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        listar();
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

    if args[0] == "--audio" {
        ouvir(&args[1..]);
        return;
    }

    let segundos: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let lista = janelas::listar();

    // Por número da lista, ou por pedaço do título. NÃO por pid: um navegador
    // tem uma dezena de janelas no mesmo processo, e foi assim que a primeira
    // versão disto escolheu a janela errada. A identidade da janela é o HWND;
    // o pid serve para o áudio, que é outra pergunta.
    let escolhida = match args[0].parse::<usize>() {
        Ok(n) if n >= 1 && n <= lista.len() => lista.get(n - 1),
        _ => {
            // Casa pelo título OU pelo executável: quem procura pensa no
            // aplicativo, e o título costuma ser outra coisa. A janela do
            // Spotify se chama pelo nome da música tocando.
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

    // Terceiro argumento liga a gravação. Sem ele, só mede a captura.
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
                // Três respostas, e não duas: "não sei" é uma delas. A versão
                // anterior afirmava "software" quando a pergunta falhava, e
                // mensagem errada custa mais caro que mensagem ausente: estava
                // acusando a CPU enquanto o chip da placa fazia o trabalho.
                println!(
                    "onde      {}",
                    match (g.em_hardware, g.codificador.as_str()) {
                        (true, _) => "CHIP DA PLACA (é o que se quer)",
                        (false, "nenhum") =>
                            "não consegui identificar (mede o tempo de CPU para saber)",
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

/// Mede o som de um processo. Sem ouvido: o que sai é nível, e nível responde
/// a pergunta que importa.
///
/// O teste do isolamento é feito com DOIS processos: um tocando e um mudo. Se
/// o mudo vier mudo com o outro tocando ao lado, a separação é real. Essa é a
/// prova que não depende de alguém escutar e achar que ouviu.
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
        if modo == audio::Modo::Incluir { "só ele" } else { "tudo MENOS ele" }
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

    println!("pedaços   {pedacos}");
    println!("amostras  {amostras}  ({:.1}s de som)", amostras as f64 / (audio::TAXA * audio::CANAIS as u32) as f64);
    println!("pico      {:.1} dB", db(pico as f64));
    println!("média     {:.1} dB", db(rms));
    println!();

    // -60 dB é silêncio de verdade: ruído de fundo digital fica bem abaixo
    // disso, e som audível fica bem acima.
    if amostras == 0 {
        println!("NADA chegou: a captura não abriu ou o processo não existe.");
    } else if db(rms) < -60.0 {
        println!("SILÊNCIO: este processo não estava emitindo som.");
    } else {
        println!("TEM SOM: a captura pegou o áudio deste processo.");
    }
}

fn listar() {
    let lista = janelas::listar();
    println!("{} janela(s) que dariam para transmitir:\n", lista.len());
    println!("{:>3}  {:<10} {:<22} {:>11}   {}", "n", "pid", "executável", "tamanho", "título");
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
        println!("(nenhuma: isso é suspeito, e vale olhar os filtros em janelas.rs)");
    } else {
        println!("\nPara capturar uma: tela-boteco <n> [segundos]");
    }
}
