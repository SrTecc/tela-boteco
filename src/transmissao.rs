//! Leva o vídeo até quem assiste, por WebRTC.
//!
//! O H.264 que saiu do chip vai **direto para a rede**, sem recodificar: o
//! navegador do outro lado decodifica o mesmo fluxo que a placa produziu. É
//! isso que mantém o custo baixo e a imagem intacta.
//!
//! Esta versão usa sinalização por arquivo, de propósito: provar que o vídeo
//! chega num navegador de verdade é um risco; falar com o site da Vercel é
//! outro. Misturar os dois faria uma falha parecer a outra.

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
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

/// Os mesmos servidores que o site usa. STUN só descobre o endereço público;
/// ele não transporta vídeo, por isso é de graça.
const STUN: &[&str] = &[
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
];

/// Transmite uma janela AO VIVO. É o produto.
///
/// A captura roda numa thread sua, o codificador na dele, e aqui só se empurra
/// para a rede o que já saiu pronto. Nenhum arquivo no caminho.
pub async fn transmitir_janela(
    hwnd: isize,
    pid: u32,
    pasta: &Path,
    segundos: f64,
    fps: u32,
    bitrate: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let (conexao, trilha, trilha_som) = montar_conexao().await?;

    // O som do processo dono da janela, codificado em Opus. Começa ANTES da
    // negociação porque a captura leva um instante para abrir, e silêncio no
    // começo da transmissão é o tipo de coisa que ninguém entende.
    let som = somente_som(pid, trilha_som, segundos);

    negociar(&conexao, pasta).await?;

    // A ponte entre o mundo sem async (captura e codificador) e o com async
    // (rede). Uma thread só, repassando: tentar fazer a captura falar async
    // seria contaminar o caminho quente com um runtime que ele não precisa.
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
/// pedaços de tamanho qualquer. Por isso existe o acumulador: juntar até fechar
/// um bloco e só então codificar. Mandar pedaço solto produz estalo.
fn somente_som(
    pid: u32,
    trilha: Arc<TrackLocalStaticSample>,
    segundos: f64,
) -> tokio::task::JoinHandle<u64> {
    tokio::spawn(async move {
        let taxa = crate::audio::TAXA;
        let canais = crate::audio::CANAIS as usize;
        // 20 ms é o bloco padrão do WebRTC: menor aumenta o custo por pacote,
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
            // Som é o motivo do app existir, mas falhar nele não derruba a
            // transmissão: imagem sem som é melhor que nada.
            eprintln!("aviso: não consegui abrir o som do processo");
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

/// A conexão e as trilhas, que são iguais nos dois modos.
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
        println!("conexão: {estado}");
        Box::pin(async {})
    }));

    Ok((conexao, trilha, trilha_som))
}

/// Troca oferta e resposta pela pasta de sinalização.
async fn negociar(
    conexao: &Arc<webrtc::peer_connection::RTCPeerConnection>,
    pasta: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let oferta = conexao.create_offer(None).await?;
    let mut coleta = conexao.gathering_complete_promise().await;
    conexao.set_local_description(oferta).await?;
    let _ = coleta.recv().await;

    let local = conexao.local_description().await.ok_or("sem descrição local")?;
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
        return Err("a amostra não tem quadro nenhum".into());
    }

    let mut motor = MediaEngine::default();
    motor.register_default_codecs()?;
    let mut registro = Registry::new();
    registro = register_default_interceptors(registro, &mut motor)?;

    // O navegador ESCONDE o IP local da pessoa atrás de um nome `.local`
    // (mDNS), por privacidade. Sem saber resolver esses nomes, nenhum caminho
    // de rede fecha e a conexão morre em "failed" sem explicar por quê: foi
    // exatamente o que aconteceu no primeiro teste.
    //
    // `QueryOnly` e não `QueryAndGather`: a gente resolve o nome do outro lado,
    // mas anuncia o próprio IP direto. Esconder o nosso não protegeria ninguém
    // (quem transmite já se identificou) e só adicionaria uma etapa que pode
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
        println!("conexão: {estado}");
        Box::pin(async {})
    }));

    // Oferta com os candidatos JÁ DENTRO: aqui não há caixa postal, então
    // esperar a coleta terminar é mais simples que mandar candidato por
    // candidato. No site será o contrário, porque lá a espera atrasaria a
    // abertura da imagem.
    let oferta = conexao.create_offer(None).await?;
    let mut coleta = conexao.gathering_complete_promise().await;
    conexao.set_local_description(oferta).await?;
    let _ = coleta.recv().await;

    let local = conexao.local_description().await.ok_or("sem descrição local")?;
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

    // Manda os quadros em círculo, no ritmo do vídeo.
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
/// Cada quadro pode ter várias unidades NAL juntas: a primeira imagem vem
/// precedida de cabeçalhos (SPS e PPS) que o decodificador precisa antes de
/// qualquer coisa. Mandar cada NAL como um quadro separado faria o navegador
/// receber cabeçalho sem imagem e imagem sem cabeçalho.
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

        // O tipo da unidade está nos 5 bits baixos do byte seguinte ao
        // marcador de início.
        let cabecalho = if pedaco.len() > 4 && pedaco[2] == 1 {
            pedaco[3]
        } else if pedaco.len() > 5 {
            pedaco[4]
        } else {
            continue;
        };
        let tipo = cabecalho & 0x1f;
        let e_imagem = tipo == 1 || tipo == 5;

        // Começa quadro novo quando chega imagem depois de já ter uma, ou
        // quando chega cabeçalho de sequência (que abre o próximo).
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
