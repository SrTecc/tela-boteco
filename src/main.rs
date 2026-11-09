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

// Executavel de JANELA, e nao de console: quem da dois cliques no app nao deve
// ver uma tela preta abrir junto. O `console::anexar` devolve a saida quando o
// app e chamado de um terminal, que e onde a medicao e o diagnostico vivem.
#![windows_subsystem = "windows"]

mod audio;
mod bandeja;
mod captura;
mod codificador;
mod console;
mod fontes;
mod interface;
mod janelas;
mod monitores;
mod painel;
mod qualidade;
mod selo;
mod tema;

/// Resolve o que transmitir a partir do que a pessoa escreveu.
///
/// Devolve o alvo, de quem e o som e um nome para mostrar. Janela leva o som
/// DAQUELE processo; monitor leva tudo menos o Discord, senao quem assiste
/// ouve a propria voz com atraso e o eco fica insuportavel em segundos.
fn escolher_fonte(texto: Option<&str>) -> Option<(captura::Alvo, audio::Som, String)> {
    // A lista e a MESMA que a interface mostra, e a traducao de indice e a
    // testada: duas listagens paralelas divergiriam na primeira correcao feita
    // so de um lado, e o jeito de notar seria transmitir a fonte errada.
    let todas = fontes::listar();
    let monitores: Vec<String> =
        todas.iter().filter(|f| f.tela_inteira).map(|f| f.rotulo.clone()).collect();
    let janelas: Vec<String> =
        todas.iter().filter(|f| !f.tela_inteira).map(|f| f.busca.clone()).collect();

    let texto = texto.unwrap_or("");
    match fontes::escolher(texto, &janelas, &monitores) {
        Some(escolha) => {
            let f = todas.get(fontes::indice_na_lista(escolha, monitores.len()))?;
            if f.tela_inteira && f.som == audio::Som::Nenhum {
                eprintln!("aviso: nao achei o Discord, entao vai sem som (a tela inteira");
                eprintln!("       com som inteiro faria quem assiste ouvir a propria voz)");
            }
            Some((f.alvo, f.som, f.rotulo.clone()))
        }
        None => {
            eprintln!("nao achei '{texto}'. o que da para transmitir:");
            for (i, r) in monitores.iter().enumerate() {
                eprintln!("  tela {:<6} {}", i + 1, r);
            }
            for f in todas.iter().filter(|f| !f.tela_inteira).take(20) {
                eprintln!("  {:<11} {}", "", f.rotulo);
            }
            None
        }
    }
}

/// O endereco do site, num lugar so.
///
/// Estava escrito em tres pontos, e a troca de share-portomaldito para
/// tela-do-boteco mostrou o preco disso: endereco repetido e endereco que
/// alguem esquece de trocar.
const SITE: &str = "https://tela-do-boteco.vercel.app";
mod sala;
mod sessao;
mod transmissao;
mod vivo;

fn main() {
    // Antes de qualquer saida: o executavel e de janela, e sem isto nada do
    // que vem abaixo apareceria num terminal.
    console::anexar();
    // Antes de perguntar o tamanho de qualquer tela.
    monitores::ajustar_dpi();
    // Liga com RUST_LOG=webrtc_ice=debug para ver por que uma conexao nao fecha.
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        // O produto: a janela. A lista no terminal continua em `--listar`.
        if let Err(e) = interface::abrir() {
            eprintln!("a janela nao abriu: {e}");
            std::process::exit(1);
        }
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
            captura::Alvo::Janela(j.hwnd),
            audio::Som::Processo(j.pid),
            &pasta, segundos, qualidade::padrao(),
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

    if args[0] == "--transmitir-sala" {
        // O caminho de verdade: a tela indo para a sala do site.
        let site = SITE;
        let Some(s) = sessao::guardada() else {
            eprintln!("sem sessao: roda --entrar primeiro");
            std::process::exit(1);
        };
        let (alvo, som, nome) = match escolher_fonte(args.get(1).map(String::as_str)) {
            Some(x) => x,
            None => std::process::exit(1),
        };
        let segundos: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60.0);
        let q = match args.get(3) {
            Some(nome) => match qualidade::por_id(nome) {
                Some(q) => q,
                None => {
                    eprintln!("qualidade '{nome}' nao existe. as que existem:");
                    for p in qualidade::PRESETS {
                        eprintln!("  {:<12} {}", p.id, p.rotulo);
                    }
                    std::process::exit(1);
                }
            },
            None => qualidade::padrao(),
        };
        println!("transmitindo '{nome}' para a sala, em {}...", q.rotulo);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        match rt.block_on(transmissao::transmitir_na_sala(
            alvo, som, site, &s, segundos, q, painel::Painel::novo(),
        )) {
            Ok(relatorio) => println!("{relatorio}"),
            Err(e) => {
                eprintln!("falhou: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if args[0] == "--sala" {
        // Conferencia da conversa com o site, antes de ligar a transmissao
        // nela: bate o ponto e le a caixa postal uma vez.
        let site = SITE;
        let Some(s) = sessao::guardada() else {
            eprintln!("sem sessao: roda --entrar primeiro");
            std::process::exit(1);
        };
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let sala = match sala::Sala::nova(site, &s) {
                Ok(x) => x,
                Err(e) => { eprintln!("falhou: {e}"); std::process::exit(1); }
            };
            match sala.bater_ponto(false).await {
                Ok(p) => {
                    println!("sala      {} ({})", p.nome.unwrap_or_default(), p.sala);
                    println!("eu        {}", p.eu);
                    println!("na sala   {} pessoa(s)", p.pessoas.len());
                    for pe in &p.pessoas {
                        println!("  - {} {}", pe.nome.clone().unwrap_or_else(|| pe.user_id.clone()),
                            if pe.esta_transmitindo() { "(transmitindo)" } else { "" });
                    }
                    match sala.ler_sinais().await {
                        Ok(v) => println!("caixa postal: {} recado(s)", v.len()),
                        Err(e) => println!("caixa postal: {e}"),
                    }
                }
                Err(e) => println!("nao deu: {e}"),
            }
        });
        return;
    }

    if args[0] == "--entrar" {
        // Login: abre o navegador e espera a sessao voltar numa porta local.
        let site = args.get(1).cloned().unwrap_or_else(|| SITE.to_string());
        match sessao::guardada() {
            Some(_) => println!("ja ha uma sessao valida guardada."),
            None => match sessao::entrar(&site) {
                Ok(_) => println!("entrou, e a sessao ficou guardada para a proxima."),
                Err(e) => { eprintln!("falhou: {e}"); std::process::exit(1); }
            },
        }
        return;
    }

    if args[0] == "--listar" {
        // O que da para transmitir, na mesma ordem e com os mesmos rotulos que
        // a escolha entende.
        match monitores::listar() {
            Ok(ms) => {
                for (i, m) in ms.iter().enumerate() {
                    // O dispositivo entra aqui porque duas telas de mesma
                    // resolucao ficam indistinguiveis so pelo rotulo, e e
                    // comum ter duas iguais na mesma mesa.
                    println!("  tela {:<4} {}  [{}]", i + 1, m.rotulo(i), m.dispositivo);
                }
            }
            Err(e) => eprintln!("nao consegui listar os monitores: {e}"),
        }
        println!();
        listar();
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

    match captura::capturar_janela(
        captura::Alvo::Janela(j.hwnd), segundos, arquivo.as_deref(), bitrate, pid_audio) {
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

/// Captura ao vivo e codifica em memória, sem arquivo no caminho.
///
/// Opcionalmente despeja os pacotes num arquivo, mas isso é conferência: serve
/// para olhar o fluxo com outra ferramenta e confirmar que saiu H.264 de
/// verdade. O produto é a tela indo para quem assiste.
fn ao_vivo(args: &[String]) {
    // Mesma resolucao da transmissao: duas listagens paralelas divergiriam na
    // primeira correcao feita so de um lado.
    let Some((alvo, _som, nome)) = escolher_fonte(args.first().map(String::as_str)) else {
        std::process::exit(1);
    };
    let segundos: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let despejo = args.get(2).cloned();

    println!("capturando '{nome}' ao vivo por {segundos}s...\n");

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

    match captura::capturar_ao_vivo(alvo, segundos, qualidade::padrao(), envio, captura::nunca()) {
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


