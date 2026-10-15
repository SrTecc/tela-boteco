//! Codificação em H.264 pelo chip de vídeo, gravando num arquivo MP4.
//!
//! O caminho é Media Foundation, e não a SDK da NVIDIA, por um motivo de
//! projeto: o dono transmite de dois PCs, um com placa dedicada e outro com
//! gráficos integrados. O Media Foundation fala com o chip dos três
//! fabricantes pelo mesmo código, então existe **um** binário em vez de três
//! caminhos para manter.
//!
//! O que importa aqui, e é o ponto do app inteiro: a textura que a captura
//! produziu vai para o codificador **sem passar pela CPU**. Quem torna isso
//! possível é o gerenciador de dispositivo DXGI, que diz ao Media Foundation
//! para trabalhar na mesma placa onde o quadro já está.
//!
//! Uma armadilha que vale conhecer: pedir codificação em hardware e **receber
//! software** é falha silenciosa. O Windows troca por um codificador de CPU sem
//! avisar, e tudo continua "funcionando", só que devagar e disputando com o
//! jogo. Por isso este módulo pergunta, depois de abrir, qual codificador
//! entrou de fato, e quem chama é obrigado a olhar a resposta.

use windows::core::{Interface, Result as WinResult, HSTRING, PCWSTR};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Media::MediaFoundation::*;

pub struct Gravador {
    escritor: IMFSinkWriter,
    fluxo: u32,
    /// O fluxo de som, quando há som. Os dois compartilham a MESMA origem de
    /// tempo: é isso que impede o áudio de andar na frente da imagem.
    fluxo_audio: Option<u32>,
    /// Textura própria para onde cada quadro é copiado. A captura recicla as
    /// dela a cada rodada; sem esta cópia (de placa para placa, que é barata),
    /// o codificador leria um quadro que já foi sobrescrito.
    destino: ID3D11Texture2D,
    contexto: ID3D11DeviceContext,
    largura: u32,
    altura: u32,
    duracao_quadro: i64,
    /// Instante do primeiro quadro, para o video comecar no zero.
    origem: Option<i64>,
    ultimo_tempo: i64,
    pub quadros: u64,
    pub amostras_audio: u64,
    pub em_hardware: bool,
    pub codificador: String,
}

/// Unidade de tempo do Media Foundation: 100 nanossegundos.
const UM_SEGUNDO: i64 = 10_000_000;

/// O gravador atravessa para a thread onde o Windows entrega os quadros.
///
/// É seguro por construção, e não por sorte: ele vive dentro de um `Mutex`,
/// então só uma thread o toca por vez, e o contexto do Direct3D (a peça que não
/// aceita dois donos ao mesmo tempo) só é usado aqui dentro. O resto são
/// objetos do Media Foundation, que são de trânsito livre entre threads.
unsafe impl Send for Gravador {}

impl Gravador {
    pub fn novo(
        d3d: &ID3D11Device,
        contexto: &ID3D11DeviceContext,
        caminho: &str,
        largura: u32,
        altura: u32,
        fps: u32,
        bitrate: u32,
        com_audio: bool,
    ) -> WinResult<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;

            // Diz ao Media Foundation em qual placa trabalhar. É isto que
            // mantém o quadro na GPU do começo ao fim.
            let mut token = 0u32;
            let mut gerenciador: Option<IMFDXGIDeviceManager> = None;
            MFCreateDXGIDeviceManager(&mut token, &mut gerenciador)?;
            let gerenciador = gerenciador.unwrap();
            gerenciador.ResetDevice(d3d, token)?;

            let mut atributos: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut atributos, 4)?;
            let atributos = atributos.unwrap();
            atributos.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            atributos.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &gerenciador)?;
            // Baixa latência: ao vivo, atraso é pior que um bit a mais.
            atributos.SetUINT32(&MF_LOW_LATENCY, 1)?;

            let escritor = MFCreateSinkWriterFromURL(
                PCWSTR(HSTRING::from(caminho).as_ptr()),
                None,
                &atributos,
            )?;

            // O que sai: H.264.
            let saida = MFCreateMediaType()?;
            saida.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            saida.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            saida.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            // Sem isto o Windows entrega Constrained Baseline, que e o
            // perfil mais pobre do H.264: com o MESMO bitrate, High rende
            // bem mais imagem. Medido: o primeiro arquivo saiu em Baseline.
            saida.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            saida.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            saida.SetUINT64(&MF_MT_FRAME_SIZE, juntar(largura, altura))?;
            saida.SetUINT64(&MF_MT_FRAME_RATE, juntar(fps, 1))?;
            saida.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, juntar(1, 1))?;
            let fluxo = escritor.AddStream(&saida)?;

            // O que entra: BGRA, que é o formato que a captura produz. A
            // conversão para NV12 (que o codificador quer) acontece na placa,
            // inserida pelo próprio Media Foundation.
            let entrada = MFCreateMediaType()?;
            entrada.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            entrada.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_ARGB32)?;
            entrada.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            entrada.SetUINT64(&MF_MT_FRAME_SIZE, juntar(largura, altura))?;
            entrada.SetUINT64(&MF_MT_FRAME_RATE, juntar(fps, 1))?;
            entrada.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, juntar(1, 1))?;
            escritor.SetInputMediaType(fluxo, &entrada, None)?;

            // O som entra como segundo fluxo, e tem de ser declarado ANTES
            // de começar a escrever: depois disso a estrutura do arquivo já
            // está fechada.
            let fluxo_audio = if com_audio {
                let saida_a = MFCreateMediaType()?;
                saida_a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                saida_a.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                saida_a.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, crate::audio::TAXA)?;
                saida_a.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, crate::audio::CANAIS as u32)?;
                saida_a.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                saida_a.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 20_000)?;
                let fa = escritor.AddStream(&saida_a)?;

                // Entra PCM de 16 bits, e não o float que a captura produz.
                // A conversão é feita aqui, em três linhas de Rust, em vez
                // de depender do Windows inserir um conversor sozinho: o
                // codificador de AAC não aceita float, e descobrir isso em
                // tempo de execução daria arquivo mudo sem erro nenhum.
                let entrada_a = MFCreateMediaType()?;
                entrada_a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                entrada_a.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                entrada_a.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, crate::audio::TAXA)?;
                entrada_a.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, crate::audio::CANAIS as u32)?;
                entrada_a.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                entrada_a.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, crate::audio::CANAIS as u32 * 2)?;
                entrada_a.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    crate::audio::TAXA * crate::audio::CANAIS as u32 * 2,
                )?;
                entrada_a.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
                escritor.SetInputMediaType(fa, &entrada_a, None)?;
                Some(fa)
            } else {
                None
            };

            escritor.BeginWriting()?;

            let (em_hardware, codificador) = quem_entrou(&escritor, fluxo);

            // A textura de trabalho, do tamanho exato do que vai ser gravado.
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: largura,
                Height: altura,
                MipLevels: 1,
                ArraySize: 1,
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut destino: Option<ID3D11Texture2D> = None;
            d3d.CreateTexture2D(&mut desc, None, Some(&mut destino))?;

            Ok(Self {
                escritor,
                fluxo,
                fluxo_audio,
                destino: destino.unwrap(),
                largura,
                altura,
                contexto: contexto.clone(),
                duracao_quadro: UM_SEGUNDO / fps as i64,
                origem: None,
                ultimo_tempo: 0,
                quadros: 0,
                amostras_audio: 0,
                em_hardware,
                codificador,
            })
        }
    }

    /// Manda um quadro. A textura continua na placa o tempo todo.
    pub fn escrever(&mut self, textura: &ID3D11Texture2D, tempo: i64) -> WinResult<()> {
        unsafe {
            // Cópia de placa para placa: tira o quadro da roda da captura, que
            // vai reaproveitar aquela textura na próxima rodada.
            //
            // É região, e não cópia inteira, porque a janela pode ter dimensão
            // ÍMPAR e o H.264 não aceita: a cor é amostrada de dois em dois
            // pixels, então largura ou altura ímpar não tem onde caber. A
            // janela medida no primeiro teste era 1297x988. Copiar a região
            // par equivale a aparar uma linha de pixels, que ninguém vê.
            let mut origem = D3D11_TEXTURE2D_DESC::default();
            textura.GetDesc(&mut origem);
            let caixa = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: origem.Width.min(self.largura),
                bottom: origem.Height.min(self.altura),
                back: 1,
            };
            self.contexto
                .CopySubresourceRegion(&self.destino, 0, 0, 0, 0, textura, 0, Some(&caixa));

            let buffer = MFCreateDXGISurfaceBuffer(
                &ID3D11Texture2D::IID,
                &self.destino,
                0,
                false,
            )?;
            // O comprimento não vem preenchido para buffer de DXGI, e sem ele o
            // escritor recusa a amostra.
            let tamanho = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(tamanho)?;

            let amostra = MFCreateSample()?;
            amostra.AddBuffer(&buffer)?;
            // O carimbo vem do INSTANTE EM QUE O QUADRO FOI CAPTURADO, e
            // nao de um contador a 60 por segundo. A captura so entrega
            // quadro quando a imagem muda, entao contar passo fixo fazia 8
            // segundos de tela virarem 2,7 segundos de video acelerado.
            let origem = *self.origem.get_or_insert(tempo);
            let agora = (tempo - origem).max(0);
            let duracao = (agora - self.ultimo_tempo).clamp(1, UM_SEGUNDO);
            amostra.SetSampleTime(agora)?;
            amostra.SetSampleDuration(duracao)?;

            self.escritor.WriteSample(self.fluxo, &amostra)?;
            self.ultimo_tempo = agora;
            self.quadros += 1;
            Ok(())
        }
    }

    /// Manda um pedaço de som, com o instante em que foi gravado.
    ///
    /// O tempo vem do mesmo relógio do vídeo (QPC em 100ns), e as duas trilhas
    /// usam a MESMA origem. Sem isso o som sai andando à frente ou atrás da
    /// imagem, que é o defeito que ninguém perdoa numa transmissão.
    pub fn escrever_audio(&mut self, amostras: &[f32], tempo: i64) -> WinResult<()> {
        let Some(fluxo) = self.fluxo_audio else {
            return Ok(());
        };
        if amostras.is_empty() {
            return Ok(());
        }

        unsafe {
            // Float para PCM de 16 bits. O `clamp` não é preciosismo: amostra
            // acima de 1.0 existe (mixagem estoura), e sem o corte ela daria a
            // volta no inteiro e viraria estalo.
            let bytes: Vec<u8> = amostras
                .iter()
                .flat_map(|a| {
                    let v = (a.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    v.to_le_bytes()
                })
                .collect();

            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut destino: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut destino, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), destino, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;

            let quadros = amostras.len() / crate::audio::CANAIS as usize;
            let duracao = quadros as i64 * UM_SEGUNDO / crate::audio::TAXA as i64;

            let amostra = MFCreateSample()?;
            amostra.AddBuffer(&buffer)?;
            let origem = *self.origem.get_or_insert(tempo);
            amostra.SetSampleTime((tempo - origem).max(0))?;
            amostra.SetSampleDuration(duracao)?;

            self.escritor.WriteSample(fluxo, &amostra)?;
            self.amostras_audio += quadros as u64;
            Ok(())
        }
    }

    pub fn fechar(&self) -> WinResult<()> {
        unsafe {
            self.escritor.Finalize()?;
            MFShutdown()?;
            Ok(())
        }
    }
}

/// Quem acabou de entrar: o chip da placa ou um codificador de CPU?
///
/// A pergunta é feita ao transformador que o escritor escolheu. Codificador de
/// hardware se identifica por um endereço de dispositivo; o de software não
/// tem nenhum, e é assim que se separam os dois sem adivinhação.
fn quem_entrou(escritor: &IMFSinkWriter, fluxo: u32) -> (bool, String) {
    unsafe {
        let Ok(ex) = escritor.cast::<IMFSinkWriterEx>() else {
            return (false, "desconhecido".into());
        };
        // A cadeia tem mais de uma peca: o conversor de cor vem antes do
        // codificador. Perguntar so ao indice 0 respondia sobre o conversor,
        // e foi assim que a primeira versao concluiu "software" por engano.
        let mut achados: Vec<String> = Vec::new();
        let mut em_hardware = false;
        for i in 0..8u32 {
            let mut categoria = windows::core::GUID::zeroed();
            let mut transformador: Option<IMFTransform> = None;
            if ex
                .GetTransformForStream(fluxo, i, Some(&mut categoria), &mut transformador)
                .is_err()
            {
                break;
            }
            let Some(t) = transformador else { break };
            let Ok(atributos) = t.cast::<IMFAttributes>() else { continue };
            let nome = texto_de(&atributos, &MFT_FRIENDLY_NAME_Attribute);
            let url = texto_de(&atributos, &MFT_ENUM_HARDWARE_URL_Attribute);
            if !url.is_empty() {
                em_hardware = true;
            }
            achados.push(format!(
                "{}{}",
                if nome.is_empty() { "sem nome".to_string() } else { nome },
                if url.is_empty() { "" } else { " [hardware]" }
            ));
        }
        (
            em_hardware,
            if achados.is_empty() { "nenhum".into() } else { achados.join(" -> ") },
        )
    }
}

/// Lê um atributo de texto. O Media Foundation aloca a string e passa a conta
/// para quem pediu, daí o `CoTaskMemFree` logo em seguida.
fn texto_de(atributos: &IMFAttributes, chave: &windows::core::GUID) -> String {
    unsafe {
        let mut ponteiro = windows::core::PWSTR::null();
        let mut tamanho = 0u32;
        if atributos
            .GetAllocatedString(chave, &mut ponteiro, &mut tamanho)
            .is_err()
        {
            return String::new();
        }
        let texto = ponteiro.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(ponteiro.0 as *const _));
        texto
    }
}

/// O Media Foundation guarda par de números num inteiro de 64 bits.
fn juntar(alto: u32, baixo: u32) -> u64 {
    ((alto as u64) << 32) | baixo as u64
}
