//! Leva o vÃ­deo atÃ© quem assiste, por WebRTC.
//!
//! O H.264 que saiu do chip vai **direto para a rede**, sem recodificar: o
//! navegador do outro lado decodifica o mesmo fluxo que a placa produziu. Ã‰
//! isso que mantÃ©m o custo baixo e a imagem intacta.
//!
//! Esta versÃ£o usa sinalizaÃ§Ã£o por arquivo, de propÃ³sito: provar que o vÃ­deo
//! chega num navegador de verdade Ã© um risco; falar com o site da Vercel Ã©
//! outro. Misturar os dois faria uma falha parecer a outra.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

/// Os mesmos servidores que o site usa. STUN sÃ³ descobre o endereÃ§o pÃºblico;
/// ele nÃ£o transporta vÃ­deo, por isso Ã© de graÃ§a.
const STUN: &[&str] = &[
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
];

/// Transmite uma janela AO VIVO. Ã‰ o produto.
///
/// A captura roda numa thread sua, o codificador na dele, e aqui sÃ³ se empurra
/// para a rede o que jÃ¡ saiu pronto. Nenhum arquivo no caminho.
pub async fn transmitir_janela(
    hwnd: isize,
    pid: u32,
    pasta: &Path,
    segundos: f64,
    fps: u32,
    bitrate: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let (conexao, trilha, trilha_som) = montar_conexao().await?;

    // O som do processo dono da janela, codificado em Opus. ComeÃ§a ANTES da
    // negociaÃ§Ã£o porque a captura leva um instante para abrir, e silÃªncio no
    // comeÃ§o da transmissÃ£o Ã© o tipo de coisa que ninguÃ©m entende.
    let som = somente_som(pid, trilha_som, segundos);

    negociar(&conexao, pasta).await?;

    // A ponte entre o mundo sem async (captura e codificador) e o com async
    // (rede). Uma thread sÃ³, repassando: tentar fazer a captura falar async
    // seria contaminar o caminho quente com um runtime que ele nÃ£o precisa.
    let (bruto_envio, bruto_recebe) = std::sync::mpsc::channel::<Vec<u8>>();
    let (rede_envio, mut rede_recebe) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(p) = bruto_recebe.recv() {
            if rede_envio.send(p).is_err() {
                break;
            }
        }
    });

    let captura = std::thread::spawn(move || {
        crate::captura::capturar_ao_vivo(hwnd, segundos, fps, bitrate, bruto_envio)
    });

    let intervalo = Duration::from_secs_f64(1.0 / fps as f64);
    let mut enviados = 0u64;
    let mut bytes = 0u64;
    while let Some(pacote) = rede_recebe.recv().await {
        bytes += pacote.len() as u64;
        trilha
            .write_sample(&Sample {
                data: pacote.into(),
                duration: intervalo,
                ..Default::default()
            })
            .await?;
        enviados += 1;
    }

    let pacotes_som = som.await.unwrap_or(0);
    let (quadros, nome, hardware) = captura
        .join()
        .map_err(|_| "a thread da captura morreu")??;
    println!("pacotes de som      {pacotes_som}");
    println!("quadros capturados  {quadros}");
    println!("pacotes enviados    {enviados}  ({:.2} MB)", bytes as f64 / 1_048_576.0);
    println!("codificador         {nome} ({})", if hardware { "chip da placa" } else { "CPU" });

    conexao.close().await?;
    Ok(())
}

/// Captura o som de um processo, codifica em Opus e manda pela trilha.
///
/// O Opus trabalha em blocos de tamanho fixo (20 ms), e a captura entrega
/// pedaÃ§os de tamanho qualquer. Por isso existe o acumulador: juntar atÃ© fechar
/// um bloco e sÃ³ entÃ£o codificar. Mandar pedaÃ§o solto produz estalo.
fn somente_som(
    pid: u32,
    trilha: Arc<TrackLocalStaticSample>,
    segundos: f64,
) -> tokio::task::JoinHandle<u64> {
    tokio::spawn(async move {
        let taxa = crate::audio::TAXA;
        let canais = crate::audio::CANAIS as usize;
        // 20 ms Ã© o bloco padrÃ£o do WebRTC: menor aumenta o custo por pacote,
        // maior aumenta o atraso.
        let por_bloco = (taxa as usize / 50) * canais;

        // Codec em Rust puro. A alternativa era a biblioteca C de referencia,
        // que nao compila aqui sem dependencia externa: para um app que as
        // pessoas vao baixar, depender de biblioteca de sistema e pedir que
        // ele falhe na maquina de alguem.
        let Ok(mut opus) = rusty_opus::OpusEncoder::new(
            taxa as i32,
            canais,
            rusty_opus::Application::Audio,
        ) else {
            eprintln!("aviso: nao consegui abrir o codificador de som");
            return 0;
        };

        let Ok(captura) = crate::audio::CapturaAudio::iniciar(pid, crate::audio::Modo::Incluir)
        else {
            // Som Ã© o motivo do app existir, mas falhar nele nÃ£o derruba a
            // transmissÃ£o: imagem sem som Ã© melhor que nada.
            eprintln!("aviso: nÃ£o consegui abrir o som do processo");
            return 0;
        };

        let fim = std::time::Instant::now() + std::time::Duration::from_secs_f64(segundos);
        let mut acumulado: Vec<f32> = Vec::with_capacity(por_bloco * 2);
        let mut enviados = 0u64;
        let duracao = Duration::from_millis(20);
        // Mede o que ENTRA no codificador. Sem isso, pacote de 5 bytes
        // (silencio em Opus) nao diz se a captura veio muda ou se o
        // codificador e que nao esta codificando.
        let mut pico_entrada = 0f32;
        let mut bytes_saida = 0u64;

        while std::time::Instant::now() < fim {
            match captura.receptor.recv_timeout(Duration::from_millis(100)) {
                Ok(p) => acumulado.extend_from_slice(&p.amostras),
                Err(_) => continue,
            }
            while acumulado.len() >= por_bloco {
                let bloco: Vec<f32> = acumulado.drain(..por_bloco).collect();
                for a in &bloco {
                    if a.abs() > pico_entrada { pico_entrada = a.abs(); }
                }
                let mut saida = vec![0u8; 4000];
                // O tamanho do bloco e por CANAL: 960 amostras sao 20 ms a
                // 48 kHz, independentemente de ser mono ou estereo.
                let por_canal = bloco.len() / canais;
                if let Ok(n) = opus.encode(&bloco, por_canal, &mut saida) {
                    bytes_saida += n as u64;
                    saida.truncate(n);
                    if trilha
                        .write_sample(&Sample {
                            data: saida.into(),
                            duration: duracao,
                            ..Default::default()
                        })
                        .await
                        .is_ok()
                    {
                        enviados += 1;
                    }
                }
            }
        }
        captura.parar();
        let db = if pico_entrada > 0.0 { 20.0 * pico_entrada.log10() } else { -120.0 };
        println!(
            "som: entrou com pico de {db:.1} dB, saiu com {} bytes em {enviados} pacotes",
            bytes_saida,
        );
        enviados
    })
}

/// A conexÃ£o e as trilhas, que sÃ£o iguais nos dois modos.
async fn montar_conexao() -> Result<
    (
        Arc<webrtc::peer_connection::RTCPeerConnection>,
        Arc<TrackLocalStaticSample>,
        Arc<TrackLocalStaticSample>,
    ),
    Box<dyn std::error::Error>,
> {
    let mut motor = MediaEngine::default();
    motor.register_default_codecs()?;
    let mut registro = Registry::new();
    registro = register_default_interceptors(registro, &mut motor)?;

    let mut ajustes = webrtc::api::setting_engine::SettingEngine::default();
    ajustes.set_ice_multicast_dns_mode(webrtc::ice::mdns::MulticastDnsMode::QueryOnly);

    let api = APIBuilder::new()
        .with_media_engine(motor)
        .with_interceptor_registry(registro)
        .with_setting_engine(ajustes)
        .build();

    let conexao = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: STUN.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await?,
    );

    let trilha = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            ..Default::default()
        },
        "video".to_owned(),
        "tela-boteco".to_owned(),
    ));
    conexao
        .add_track(Arc::clone(&trilha) as Arc<dyn TrackLocal + Send + Sync>)
        .await?;

    // A trilha de som. Opus porque e o codec do WebRTC: o AAC do gravador
    // nao serve aqui, navegador nenhum aceita AAC nesse caminho.
    let trilha_som = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48_000,
            channels: 2,
            ..Default::default()
        },
        "som".to_owned(),
        "tela-boteco".to_owned(),
    ));
    conexao
        .add_track(Arc::clone(&trilha_som) as Arc<dyn TrackLocal + Send + Sync>)
        .await?;

    conexao.on_peer_connection_state_change(Box::new(|estado: RTCPeerConnectionState| {
        println!("conexÃ£o: {estado}");
        Box::pin(async {})
    }));

    Ok((conexao, trilha, trilha_som))
}

/// Troca oferta e resposta pela pasta de sinalizaÃ§Ã£o.
async fn negociar(
    conexao: &Arc<webrtc::peer_connection::RTCPeerConnection>,
    pasta: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let oferta = conexao.create_offer(None).await?;
    let mut coleta = conexao.gathering_complete_promise().await;
    conexao.set_local_description(oferta).await?;
    let _ = coleta.recv().await;

    let local = conexao.local_description().await.ok_or("sem descriÃ§Ã£o local")?;
    let arquivo_resposta = pasta.join("resposta.json");
    let _ = std::fs::remove_file(&arquivo_resposta);
    std::fs::write(pasta.join("oferta.json"), serde_json::to_string(&local)?)?;
    println!("oferta escrita, esperando a resposta...");

    let resposta = loop {
        if let Ok(texto) = std::fs::read_to_string(&arquivo_resposta) {
            if !texto.trim().is_empty() {
                break texto;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    conexao
        .set_remote_description(serde_json::from_str(&resposta)?)
        .await?;
    println!("resposta recebida, negociando...");
    Ok(())
}

pub async fn transmitir_arquivo(
    caminho: &Path,
    pasta: &Path,
    fps: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let bruto = std::fs::read(caminho)?;
    let unidades = separar_em_quadros(&bruto);
    println!("amostra: {} quadros de H.264", unidades.len());
    if unidades.is_empty() {
        return Err("a amostra nÃ£o tem quadro nenhum".into());
    }

    let mut motor = MediaEngine::default();
    motor.register_default_codecs()?;
    let mut registro = Registry::new();
    registro = register_default_interceptors(registro, &mut motor)?;

    // O navegador ESCONDE o IP local da pessoa atrÃ¡s de um nome `.local`
    // (mDNS), por privacidade. Sem saber resolver esses nomes, nenhum caminho
    // de rede fecha e a conexÃ£o morre em "failed" sem explicar por quÃª: foi
    // exatamente o que aconteceu no primeiro teste.
    //
    // `QueryOnly` e nÃ£o `QueryAndGather`: a gente resolve o nome do outro lado,
    // mas anuncia o prÃ³prio IP direto. Esconder o nosso nÃ£o protegeria ninguÃ©m
    // (quem transmite jÃ¡ se identificou) e sÃ³ adicionaria uma etapa que pode
    // falhar.
    let mut ajustes = webrtc::api::setting_engine::SettingEngine::default();
    ajustes.set_ice_multicast_dns_mode(webrtc::ice::mdns::MulticastDnsMode::QueryOnly);

    let api = APIBuilder::new()
        .with_media_engine(motor)
        .with_interceptor_registry(registro)
        .with_setting_engine(ajustes)
        .build();

    let conexao = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: STUN.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await?,
    );

    let trilha = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            ..Default::default()
        },
        "video".to_owned(),
        "tela-boteco".to_owned(),
    ));
    conexao
        .add_track(Arc::clone(&trilha) as Arc<dyn TrackLocal + Send + Sync>)
        .await?;

    conexao.on_peer_connection_state_change(Box::new(|estado: RTCPeerConnectionState| {
        println!("conexÃ£o: {estado}");
        Box::pin(async {})
    }));

    // Oferta com os candidatos JÃ DENTRO: aqui nÃ£o hÃ¡ caixa postal, entÃ£o
    // esperar a coleta terminar Ã© mais simples que mandar candidato por
    // candidato. No site serÃ¡ o contrÃ¡rio, porque lÃ¡ a espera atrasaria a
    // abertura da imagem.
    let oferta = conexao.create_offer(None).await?;
    let mut coleta = conexao.gathering_complete_promise().await;
    conexao.set_local_description(oferta).await?;
    let _ = coleta.recv().await;

    let local = conexao.local_description().await.ok_or("sem descriÃ§Ã£o local")?;
    let arquivo_oferta = pasta.join("oferta.json");
    let arquivo_resposta = pasta.join("resposta.json");
    let _ = std::fs::remove_file(&arquivo_resposta);
    std::fs::write(&arquivo_oferta, serde_json::to_string(&local)?)?;
    println!("oferta escrita em {}", arquivo_oferta.display());

    // Espera a resposta de quem assiste.
    println!("esperando a resposta...");
    let resposta = loop {
        if let Ok(texto) = std::fs::read_to_string(&arquivo_resposta) {
            if !texto.trim().is_empty() {
                break texto;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    let resposta: RTCSessionDescription = serde_json::from_str(&resposta)?;
    conexao.set_remote_description(resposta).await?;
    println!("resposta recebida, negociando...");

    // Manda os quadros em cÃ­rculo, no ritmo do vÃ­deo.
    let intervalo = Duration::from_secs_f64(1.0 / fps as f64);
    let mut relogio = tokio::time::interval(intervalo);
    let mut enviados = 0u64;
    let inicio = std::time::Instant::now();

    while inicio.elapsed() < Duration::from_secs(20) {
        for quadro in &unidades {
            relogio.tick().await;
            trilha
                .write_sample(&Sample {
                    data: quadro.clone().into(),
                    duration: intervalo,
                    ..Default::default()
                })
                .await?;
            enviados += 1;
        }
    }

    println!("enviados {enviados} quadros em {:.1}s", inicio.elapsed().as_secs_f64());
    conexao.close().await?;
    Ok(())
}

/// Separa o fluxo Annex-B em quadros (unidades de acesso).
///
/// Cada quadro pode ter vÃ¡rias unidades NAL juntas: a primeira imagem vem
/// precedida de cabeÃ§alhos (SPS e PPS) que o decodificador precisa antes de
/// qualquer coisa. Mandar cada NAL como um quadro separado faria o navegador
/// receber cabeÃ§alho sem imagem e imagem sem cabeÃ§alho.
pub fn separar_em_quadros(bruto: &[u8]) -> Vec<Vec<u8>> {
    let mut inicios: Vec<usize> = Vec::new();
    let mut i = 0usize;
    while i + 3 < bruto.len() {
        if bruto[i] == 0 && bruto[i + 1] == 0 && bruto[i + 2] == 1 {
            inicios.push(i);
            i += 3;
        } else if i + 4 < bruto.len()
            && bruto[i] == 0
            && bruto[i + 1] == 0
            && bruto[i + 2] == 0
            && bruto[i + 3] == 1
        {
            inicios.push(i);
            i += 4;
        } else {
            i += 1;
        }
    }

    let mut quadros: Vec<Vec<u8>> = Vec::new();
    let mut atual: Vec<u8> = Vec::new();
    let mut ja_tem_imagem = false;

    for (n, &inicio) in inicios.iter().enumerate() {
        let fim = inicios.get(n + 1).copied().unwrap_or(bruto.len());
        let pedaco = &bruto[inicio..fim];

        // O tipo da unidade estÃ¡ nos 5 bits baixos do byte seguinte ao
        // marcador de inÃ­cio.
        let cabecalho = if pedaco.len() > 4 && pedaco[2] == 1 {
            pedaco[3]
        } else if pedaco.len() > 5 {
            pedaco[4]
        } else {
            continue;
        };
        let tipo = cabecalho & 0x1f;
        let e_imagem = tipo == 1 || tipo == 5;

        // ComeÃ§a quadro novo quando chega imagem depois de jÃ¡ ter uma, ou
        // quando chega cabeÃ§alho de sequÃªncia (que abre o prÃ³ximo).
        if (e_imagem && ja_tem_imagem) || (tipo == 7 && ja_tem_imagem) {
            quadros.push(std::mem::take(&mut atual));
            ja_tem_imagem = false;
        }
        atual.extend_from_slice(pedaco);
        if e_imagem {
            ja_tem_imagem = true;
        }
    }
    if !atual.is_empty() {
        quadros.push(atual);
    }
    quadros
}

/// Transmite para a sala do site. Ãƒâ€° o caminho de verdade.
///
/// A diferenÃƒÂ§a para o modo de teste nÃƒÂ£o ÃƒÂ© a captura nem a codificaÃƒÂ§ÃƒÂ£o: ÃƒÂ© a
/// sinalizaÃƒÂ§ÃƒÂ£o. Aqui ela passa pela caixa postal da Vercel, com **candidatos
/// em fluxo** (cada um vai assim que nasce) em vez de esperar a coleta
/// terminar. Esperar funcionaria, mas atrasaria a abertura da imagem em alguns
/// segundos, e quem estÃƒÂ¡ do outro lado olhando uma tela preta nÃƒÂ£o sabe se
/// quebrou.
///
/// E sÃƒÂ£o **vÃƒÂ¡rios espectadores**: uma conexÃƒÂ£o por pessoa, com as mesmas duas
/// trilhas. Ãƒâ€° por isso que o limite da Tela ÃƒÂ© o upload de quem transmite.
pub async fn transmitir_na_sala(
    hwnd: isize,
    pid: u32,
    site: &str,
    sessao: &str,
    segundos: f64,
    fps: u32,
    bitrate: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let sala = Arc::new(crate::sala::Sala::nova(site, sessao)?);

    // Pede a tela ANTES de montar qualquer coisa: se outra pessoa jÃƒÂ¡ estÃƒÂ¡ no
    // ar, nÃƒÂ£o hÃƒÂ¡ por que ligar cÃƒÂ¢mera nenhuma.
    let presenca = sala.bater_ponto(true).await?;
    if !presenca.tela_livre() {
        return Err("outra pessoa jÃƒÂ¡ estÃƒÂ¡ transmitindo nesta sala".into());
    }
    println!(
        "sala: {} ({} pessoa(s))",
        presenca.nome.clone().unwrap_or_default(),
        presenca.pessoas.len()
    );

    let (api, trilha, trilha_som) = montar_fabrica().await?;
    let som = somente_som(pid, Arc::clone(&trilha_som), segundos);

    // A captura ÃƒÂ© uma sÃƒÂ³, e alimenta todas as conexÃƒÂµes: o quadro ÃƒÂ© codificado
    // UMA vez e a trilha o entrega a quem estiver ligado. Codificar por
    // espectador multiplicaria o custo sem motivo.
    let (bruto_envio, bruto_recebe) = std::sync::mpsc::channel::<Vec<u8>>();
    let (rede_envio, mut rede_recebe) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(p) = bruto_recebe.recv() {
            if rede_envio.send(p).is_err() {
                break;
            }
        }
    });
    let captura = std::thread::spawn(move || {
        crate::captura::capturar_ao_vivo(hwnd, segundos, fps, bitrate, bruto_envio)
    });

    // A batida de presenÃƒÂ§a continua durante toda a transmissÃƒÂ£o: ÃƒÂ© ela que
    // prova ao site, a cada ciclo, que a pessoa continua na call.
    let ponto = {
        let sala = Arc::clone(&sala);
        let ate = std::time::Instant::now() + Duration::from_secs_f64(segundos);
        tokio::spawn(async move {
            while std::time::Instant::now() < ate {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if let Err(e) = sala.bater_ponto(true).await {
                    eprintln!("presenÃƒÂ§a: {e}");
                }
            }
        })
    };

    // Uma conexÃƒÂ£o por espectador.
    let conexoes: Arc<tokio::sync::Mutex<HashMap<String, Arc<RTCPeerConnection>>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let sinais = {
        let sala = Arc::clone(&sala);
        let conexoes = Arc::clone(&conexoes);
        let trilha = Arc::clone(&trilha);
        let trilha_som = Arc::clone(&trilha_som);
        let ate = std::time::Instant::now() + Duration::from_secs_f64(segundos);
        tokio::spawn(async move {
            while std::time::Instant::now() < ate {
                match sala.ler_sinais().await {
                    Ok(recados) => {
                        for recado in recados {
                            if let Err(e) = tratar_recado(
                                &recado, &sala, &conexoes, &api, &trilha, &trilha_som,
                            )
                            .await
                            {
                                eprintln!("sinal {}: {e}", recado.tipo);
                            }
                        }
                    }
                    Err(e) => eprintln!("caixa postal: {e}"),
                }
                tokio::time::sleep(Duration::from_millis(1200)).await;
            }
        })
    };

    // Empurra o vÃƒÂ­deo para as trilhas. Uma trilha sÃƒÂ³, vÃƒÂ¡rios destinos.
    let intervalo = Duration::from_secs_f64(1.0 / fps as f64);
    let mut enviados = 0u64;
    while let Some(pacote) = rede_recebe.recv().await {
        trilha
            .write_sample(&Sample {
                data: pacote.into(),
                duration: intervalo,
                ..Default::default()
            })
            .await?;
        enviados += 1;
    }

    sinais.abort();
    ponto.abort();
    let pacotes_som = som.await.unwrap_or(0);
    let (quadros, nome, hardware) = captura.join().map_err(|_| "a captura morreu")??;

    // Avisa quem estÃƒÂ¡ ligado que acabou, e solta a tela no site. Sem isso a
    // sala fica com um transmissor fantasma atÃƒÂ© a poda.
    let mapa = conexoes.lock().await;
    for (quem, pc) in mapa.iter() {
        let _ = sala.mandar(quem, "saiu", serde_json::json!({})).await;
        let _ = pc.close().await;
    }
    drop(mapa);
    sala.sair().await;

    println!("quadros capturados  {quadros}");
    println!("pacotes de video    {enviados}");
    println!("pacotes de som      {pacotes_som}");
    println!("codificador         {nome} ({})", if hardware { "chip da placa" } else { "CPU" });
    Ok(())
}

/// Um recado da caixa postal.
async fn tratar_recado(
    recado: &crate::sala::Sinal,
    sala: &Arc<crate::sala::Sala>,
    conexoes: &Arc<tokio::sync::Mutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    api: &Arc<webrtc::api::API>,
    trilha: &Arc<TrackLocalStaticSample>,
    trilha_som: &Arc<TrackLocalStaticSample>,
) -> Result<(), Box<dyn std::error::Error>> {
    match recado.tipo.as_str() {
        // "quero" ÃƒÂ© quem chegou pedindo a tela. Quem oferece ÃƒÂ© sempre quem
        // transmite: do outro lado, uma conexÃƒÂ£o antiga parece viva, e ninguÃƒÂ©m
        // teria motivo para oferecer.
        "quero" => {
            let mut mapa = conexoes.lock().await;
            if mapa.contains_key(&recado.de) {
                return Ok(());
            }
            let pc = criar_conexao(api, trilha, trilha_som, sala, &recado.de).await?;
            let oferta = pc.create_offer(None).await?;
            pc.set_local_description(oferta).await?;
            if let Some(local) = pc.local_description().await {
                sala.mandar(
                    &recado.de,
                    "oferta",
                    serde_json::json!({ "sdp": local.sdp, "tipo": "offer" }),
                )
                .await?;
            }
            mapa.insert(recado.de.clone(), pc);
            println!("oferta mandada para {}", recado.de);
        }
        "resposta" => {
            let mapa = conexoes.lock().await;
            if let Some(pc) = mapa.get(&recado.de) {
                let sdp = recado.dados.get("sdp").and_then(|v| v.as_str()).unwrap_or_default();
                let desc = RTCSessionDescription::answer(sdp.to_string())?;
                pc.set_remote_description(desc).await?;
                println!("resposta de {} aceita", recado.de);
            }
        }
        "candidato" => {
            let mapa = conexoes.lock().await;
            if let Some(pc) = mapa.get(&recado.de) {
                let c = webrtc::ice_transport::ice_candidate::RTCIceCandidateInit {
                    candidate: recado.dados.get("candidate").and_then(|v| v.as_str())
                        .unwrap_or_default().to_string(),
                    sdp_mid: recado.dados.get("sdpMid").and_then(|v| v.as_str()).map(String::from),
                    sdp_mline_index: recado.dados.get("sdpMLineIndex")
                        .and_then(|v| v.as_u64()).map(|n| n as u16),
                    username_fragment: None,
                };
                // Candidato que chega antes da descriÃƒÂ§ÃƒÂ£o remota nÃƒÂ£o ÃƒÂ© erro: ÃƒÂ©
                // ordem de chegada. O webrtc-rs guarda sozinho nesse caso.
                let _ = pc.add_ice_candidate(c).await;
            }
        }
        "saiu" => {
            let mut mapa = conexoes.lock().await;
            if let Some(pc) = mapa.remove(&recado.de) {
                let _ = pc.close().await;
                println!("{} saiu", recado.de);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Uma conexÃƒÂ£o nova para um espectador, jÃƒÂ¡ com as trilhas e o envio de
/// candidatos em fluxo.
async fn criar_conexao(
    api: &Arc<webrtc::api::API>,
    trilha: &Arc<TrackLocalStaticSample>,
    trilha_som: &Arc<TrackLocalStaticSample>,
    sala: &Arc<crate::sala::Sala>,
    para: &str,
) -> Result<Arc<RTCPeerConnection>, Box<dyn std::error::Error>> {
    let pc = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: STUN.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await?,
    );
    pc.add_track(Arc::clone(trilha) as Arc<dyn TrackLocal + Send + Sync>).await?;
    pc.add_track(Arc::clone(trilha_som) as Arc<dyn TrackLocal + Send + Sync>).await?;

    // Candidatos EM FLUXO: cada um sai assim que nasce. Esperar a coleta
    // terminar atrasaria a imagem em segundos, e tela preta sem explicaÃƒÂ§ÃƒÂ£o ÃƒÂ©
    // pior que imagem que demora um instante a mais para ficar boa.
    {
        let sala = Arc::clone(sala);
        let para = para.to_string();
        pc.on_ice_candidate(Box::new(move |c| {
            let sala = Arc::clone(&sala);
            let para = para.clone();
            Box::pin(async move {
                let Some(c) = c else { return };
                let Ok(j) = c.to_json() else { return };
                let _ = sala
                    .mandar(
                        &para,
                        "candidato",
                        serde_json::json!({
                            "candidate": j.candidate,
                            "sdpMid": j.sdp_mid,
                            "sdpMLineIndex": j.sdp_mline_index,
                        }),
                    )
                    .await;
            })
        }));
    }

    let quem = para.to_string();
    pc.on_peer_connection_state_change(Box::new(move |estado: RTCPeerConnectionState| {
        println!("conexÃƒÂ£o com {quem}: {estado}");
        Box::pin(async {})
    }));

    Ok(pc)
}

/// A fÃƒÂ¡brica de conexÃƒÂµes e as duas trilhas, compartilhadas por todos.
async fn montar_fabrica() -> Result<
    (
        Arc<webrtc::api::API>,
        Arc<TrackLocalStaticSample>,
        Arc<TrackLocalStaticSample>,
    ),
    Box<dyn std::error::Error>,
> {
    let mut motor = MediaEngine::default();
    motor.register_default_codecs()?;
    let mut registro = Registry::new();
    registro = register_default_interceptors(registro, &mut motor)?;
    let mut ajustes = webrtc::api::setting_engine::SettingEngine::default();
    ajustes.set_ice_multicast_dns_mode(webrtc::ice::mdns::MulticastDnsMode::QueryOnly);

    let api = Arc::new(
        APIBuilder::new()
            .with_media_engine(motor)
            .with_interceptor_registry(registro)
            .with_setting_engine(ajustes)
            .build(),
    );

    let trilha = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability { mime_type: MIME_TYPE_H264.to_owned(), ..Default::default() },
        "video".to_owned(),
        "tela-boteco".to_owned(),
    ));
    let trilha_som = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48_000,
            channels: 2,
            ..Default::default()
        },
        "som".to_owned(),
        "tela-boteco".to_owned(),
    ));
    Ok((api, trilha, trilha_som))
}


