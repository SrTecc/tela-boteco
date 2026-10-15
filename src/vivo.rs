//! Codificação ao vivo: entrega o H.264 **em memória**, pacote a pacote.
//!
//! O gravador de MP4 (`codificador.rs`) provou que o chip funciona, mas ele
//! engole os pacotes dentro de um arquivo. Para transmitir é preciso a mesma
//! codificação com os bytes na mão, e isso exige falar com o codificador
//! diretamente, sem o escritor de arquivo no meio.
//!
//! Duas peças, e as duas ficam na placa:
//!
//! 1. **O conversor.** A captura entrega BGRA; o codificador quer NV12. A
//!    conversão é feita pelo processador de vídeo do Direct3D, que também
//!    **redimensiona** de graça. É por isso que ele não é um mal necessário: é
//!    ele que vai servir os presets de qualidade, quando a pessoa pedir 720p
//!    numa tela de 1440p.
//! 2. **O codificador.** Codificador de hardware no Windows é assíncrono: ele
//!    avisa quando quer entrada e quando tem saída, em vez de responder na
//!    hora. Tratar isso como função que devolve na hora é o erro clássico, e
//!    resulta em travar esperando um quadro que nunca vem.

use windows::core::{Interface, Result as WinResult, GUID};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;

pub struct Vivo {
    conversor: Conversor,
    mft: IMFTransform,
    eventos: Option<IMFMediaEventGenerator>,
    /// Quantos quadros o codificador está pedindo agora. Mandar sem ele pedir
    /// é o jeito de encher a fila dele e aumentar o atraso.
    querendo: i32,
    duracao: i64,
    proximo: i64,
    /// O quadro mais recente esperando a vez. Um so, de proposito.
    pendente: Option<ID3D11Texture2D>,
    pub nome: String,
    pub em_hardware: bool,
    /// Codificador de hardware costuma ser assincrono (avisa quando quer
    /// entrada), mas nem sempre. Tratar um sincrono como assincrono trava: ele
    /// nunca pede nada, e quem espera o pedido nunca manda quadro.
    pub assincrono: bool,
    /// Contadores de diagnostico. Existem porque adivinhar por que um
    /// codificador fica mudo ja custou duas rodadas de teste.
    pub entradas_ok: u64,
    pub entradas_erro: u64,
    pub ultimo_erro: String,
    pub renegociacoes: u64,
}

const UM_SEGUNDO: i64 = 10_000_000;

// Os dois avisos que o codificador assíncrono manda. Não têm nome na crate.
const PRECISA_ENTRADA: u32 = 601;
const TEM_SAIDA: u32 = 602;

unsafe impl Send for Vivo {}

impl Vivo {
    pub fn novo(
        d3d: &ID3D11Device,
        contexto: &ID3D11DeviceContext,
        largura: u32,
        altura: u32,
        fps: u32,
        bitrate: u32,
    ) -> WinResult<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;

            let conversor = Conversor::novo(d3d, contexto, largura, altura)?;
            let (mft, nome, em_hardware) = achar_codificador(largura, altura)?;

            // Diz em qual placa trabalhar. Sem isto o codificador exige o
            // quadro na memória da CPU, e aí todo o cuidado anterior se perde.
            let gerenciador = {
                let mut token = 0u32;
                let mut g: Option<IMFDXGIDeviceManager> = None;
                MFCreateDXGIDeviceManager(&mut token, &mut g)?;
                let g = g.unwrap();
                g.ResetDevice(d3d, token)?;
                g
            };
            let atributos = mft.GetAttributes()?;
            let assincrono = atributos.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1;
            // Codificador de hardware vem "trancado" até quem o abre avisar
            // que sabe lidar com o modo assíncrono. Falhar aqui é fatal e
            // silencioso: ele aceita tudo depois e nunca pede quadro.
            if assincrono {
                atributos.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
            mft.ProcessMessage(
                MFT_MESSAGE_SET_D3D_MANAGER,
                gerenciador.as_raw() as *const std::ffi::c_void as usize,
            )?;

            // A SAÍDA vem antes da entrada: o codificador decide o que aceita
            // na entrada a partir do que foi pedido na saída.
            let saida = MFCreateMediaType()?;
            saida.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            saida.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            saida.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            saida.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            saida.SetUINT64(&MF_MT_FRAME_SIZE, juntar(largura, altura))?;
            saida.SetUINT64(&MF_MT_FRAME_RATE, juntar(fps, 1))?;
            saida.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, juntar(1, 1))?;
            saida.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            mft.SetOutputType(0, &saida, 0)?;

            let entrada = MFCreateMediaType()?;
            entrada.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            entrada.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            entrada.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            entrada.SetUINT64(&MF_MT_FRAME_SIZE, juntar(largura, altura))?;
            entrada.SetUINT64(&MF_MT_FRAME_RATE, juntar(fps, 1))?;
            entrada.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, juntar(1, 1))?;
            mft.SetInputType(0, &entrada, 0)?;

            let eventos: Option<IMFMediaEventGenerator> =
                if assincrono { mft.cast().ok() } else { None };
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            Ok(Self {
                conversor,
                mft,
                eventos,
                querendo: 0,
                duracao: UM_SEGUNDO / fps as i64,
                proximo: 0,
                pendente: None,
                nome,
                em_hardware,
                assincrono,
                entradas_ok: 0,
                entradas_erro: 0,
                ultimo_erro: String::new(),
                renegociacoes: 0,
            })
        }
    }

    /// Guarda um quadro para o codificador pegar quando pedir.
    ///
    /// Guardar UM, e nao enfileirar: ao vivo, quadro velho nao interessa a
    /// ninguem. Se o codificador esta atrasado, o certo e ele pular para o
    /// mais recente, e nao acumular uma fila que vira atraso.
    ///
    /// Devolve os pacotes de H.264 que saíram **nesta** passada, que podem ser
    /// nenhum: o codificador trabalha com alguns quadros de atraso, e exigir
    /// resposta imediata seria travar a captura.
    pub fn oferecer(&mut self, textura: &ID3D11Texture2D) -> WinResult<()> {
        let nv12 = self.conversor.converter(textura)?;
        self.pendente = Some(nv12);
        Ok(())
    }

    /// Roda o codificador: entrega o que ele pedir e recolhe o que ele tiver.
    ///
    /// Vive numa bomba propria, e nao no caminho do quadro, porque os avisos
    /// dele chegam quando ele quer. Perguntar so na chegada de quadro era
    /// perguntar uma vez a cada mudanca de tela, cedo demais e raramente.
    pub fn bombear(&mut self) -> WinResult<Vec<Vec<u8>>> {
        if !self.assincrono {
            let Some(nv12) = self.pendente.take() else { return Ok(Vec::new()) };
            self.alimentar(&nv12)?;
            let mut pacotes = Vec::new();
            while let Some(p) = self.recolher()? {
                pacotes.push(p);
            }
            return Ok(pacotes);
        }

        let mut pacotes = self.ler_avisos()?;
        if self.querendo > 0 {
            if let Some(nv12) = self.pendente.take() {
                self.alimentar(&nv12)?;
                self.querendo -= 1;
            }
        }
        pacotes.extend(self.ler_avisos()?);
        Ok(pacotes)
    }

    fn alimentar(&mut self, nv12: &ID3D11Texture2D) -> WinResult<()> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12, 0, false)?;
            let tamanho = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(tamanho)?;
            let amostra = MFCreateSample()?;
            amostra.AddBuffer(&buffer)?;
            amostra.SetSampleTime(self.proximo)?;
            amostra.SetSampleDuration(self.duracao)?;
            match self.mft.ProcessInput(0, &amostra, 0) {
                Ok(()) => self.entradas_ok += 1,
                Err(e) => {
                    self.entradas_erro += 1;
                    self.ultimo_erro = format!("entrada: {e}");
                }
            }
        }
        self.proximo += self.duracao;
        Ok(())
    }

    #[allow(dead_code)]
    fn antigo_quadro(&mut self, textura: &ID3D11Texture2D) -> WinResult<Vec<Vec<u8>>> {
        if !self.assincrono {
            // Sincrono: manda e colhe na hora, sem esperar aviso nenhum.
            let nv12 = self.conversor.converter(textura)?;
            unsafe {
                let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &nv12, 0, false)?;
                let tamanho = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
                buffer.SetCurrentLength(tamanho)?;
                let amostra = MFCreateSample()?;
                amostra.AddBuffer(&buffer)?;
                amostra.SetSampleTime(self.proximo)?;
                amostra.SetSampleDuration(self.duracao)?;
                match self.mft.ProcessInput(0, &amostra, 0) {
                    Ok(()) => self.entradas_ok += 1,
                    Err(e) => {
                        self.entradas_erro += 1;
                        self.ultimo_erro = format!("entrada: {e}");
                    }
                }
            }
            self.proximo += self.duracao;
            let mut pacotes = Vec::new();
            // Drena tudo que estiver pronto: um quadro de entrada pode nao
            // render saida nenhuma, e depois render duas.
            while let Some(p) = self.recolher()? {
                pacotes.push(p);
            }
            return Ok(pacotes);
        }

        self.ler_avisos()?;

        if self.querendo > 0 {
            let nv12 = self.conversor.converter(textura)?;
            unsafe {
                let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &nv12, 0, false)?;
                let tamanho = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
                buffer.SetCurrentLength(tamanho)?;

                let amostra = MFCreateSample()?;
                amostra.AddBuffer(&buffer)?;
                amostra.SetSampleTime(self.proximo)?;
                amostra.SetSampleDuration(self.duracao)?;
                self.mft.ProcessInput(0, &amostra, 0)?;
            }
            self.proximo += self.duracao;
            self.querendo -= 1;
        }

        self.ler_avisos()
    }

    /// Lê os avisos pendentes e recolhe a saída pronta.
    fn ler_avisos(&mut self) -> WinResult<Vec<Vec<u8>>> {
        let mut pacotes = Vec::new();
        let Some(eventos) = self.eventos.clone() else { return Ok(pacotes) };
        loop {
            // Sem espera: este laço roda no caminho do quadro, e parar aqui
            // seria segurar a captura inteira.
            let evento = unsafe { eventos.GetEvent(MF_EVENT_FLAG_NO_WAIT) };
            let Ok(evento) = evento else { break };
            let tipo = unsafe { evento.GetType()? };
            match tipo {
                PRECISA_ENTRADA => self.querendo += 1,
                TEM_SAIDA => {
                    if let Some(bytes) = self.recolher()? {
                        pacotes.push(bytes);
                    }
                }
                _ => {}
            }
        }
        Ok(pacotes)
    }

    /// Tira um pacote codificado de dentro do codificador.
    fn recolher(&mut self) -> WinResult<Option<Vec<u8>>> {
        unsafe {
            let mut saida = [MFT_OUTPUT_DATA_BUFFER::default(); 1];
            let mut estado = 0u32;
            // Codificador de hardware entrega a amostra pronta: não é preciso
            // oferecer buffer, ele traz o dele.
            if let Err(e) = self.mft.ProcessOutput(0, &mut saida, &mut estado) {
                // "precisa de mais entrada" e o caso normal: o codificador
                // trabalha com atraso.
                if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    return Ok(None);
                }
                // "mudanca de fluxo" TAMBEM e normal, e e parte do protocolo:
                // o codificador escolheu o formato de saida de verdade (com o
                // cabecalho de sequencia dentro) e exige que a gente aceite o
                // novo antes de continuar. Quem nao renegocia fica preso aqui
                // para sempre, sem um unico pacote, exatamente como aconteceu.
                if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                    if let Ok(novo) = self.mft.GetOutputAvailableType(0, 0) {
                        if let Err(e2) = self.mft.SetOutputType(0, &novo, 0) {
                            self.ultimo_erro = format!("renegociar: {e2}");
                        } else {
                            self.renegociacoes += 1;
                        }
                    }
                    return Ok(None);
                }
                self.ultimo_erro = format!("saida: {e}");
                return Ok(None);
            }
            let Some(amostra) = saida[0].pSample.take() else {
                return Ok(None);
            };

            let buffer = amostra.ConvertToContiguousBuffer()?;
            let mut dados: *mut u8 = std::ptr::null_mut();
            let mut comprimento = 0u32;
            buffer.Lock(&mut dados, None, Some(&mut comprimento))?;
            let bytes = std::slice::from_raw_parts(dados, comprimento as usize).to_vec();
            buffer.Unlock()?;
            Ok(Some(bytes))
        }
    }
}

/// Converte BGRA para NV12 na própria placa, e redimensiona no caminho.
struct Conversor {
    contexto: ID3D11VideoContext,
    processador: ID3D11VideoProcessor,
    enumerador: ID3D11VideoProcessorEnumerator,
    d3d: ID3D11Device,
    destino: ID3D11Texture2D,
    saida: Option<ID3D11VideoProcessorOutputView>,
}

impl Conversor {
    fn novo(
        d3d: &ID3D11Device,
        contexto: &ID3D11DeviceContext,
        largura: u32,
        altura: u32,
    ) -> WinResult<Self> {
        unsafe {
            let video: ID3D11VideoDevice = d3d.cast()?;
            let video_ctx: ID3D11VideoContext = contexto.cast()?;

            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: largura,
                InputHeight: altura,
                OutputWidth: largura,
                OutputHeight: altura,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };
            let enumerador = video.CreateVideoProcessorEnumerator(&desc)?;
            let processador = video.CreateVideoProcessor(&enumerador, 0)?;

            let mut nv12 = D3D11_TEXTURE2D_DESC {
                Width: largura,
                Height: altura,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                ..Default::default()
            };
            let mut destino: Option<ID3D11Texture2D> = None;
            d3d.CreateTexture2D(&mut nv12, None, Some(&mut destino))?;

            Ok(Self {
                contexto: video_ctx,
                processador,
                enumerador,
                d3d: d3d.clone(),
                destino: destino.unwrap(),
                saida: None,
            })
        }
    }

    fn converter(&mut self, origem: &ID3D11Texture2D) -> WinResult<ID3D11Texture2D> {
        unsafe {
            let video: ID3D11VideoDevice = self.d3d.cast()?;

            if self.saida.is_none() {
                let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };
                let mut v: Option<ID3D11VideoProcessorOutputView> = None;
                video.CreateVideoProcessorOutputView(
                    &self.destino,
                    &self.enumerador,
                    &desc,
                    Some(&mut v),
                )?;
                self.saida = v;
            }

            let entrada_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            let mut entrada: Option<ID3D11VideoProcessorInputView> = None;
            video.CreateVideoProcessorInputView(
                origem,
                &self.enumerador,
                &entrada_desc,
                Some(&mut entrada),
            )?;

            let fluxo = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::transmute_copy(&entrada),
                ..Default::default()
            };
            self.contexto.VideoProcessorBlt(
                &self.processador,
                self.saida.as_ref().unwrap(),
                0,
                &[fluxo],
            )?;
            Ok(self.destino.clone())
        }
    }
}

/// Procura um codificador de H.264 que rode no chip da placa.
fn achar_codificador(_largura: u32, _altura: u32) -> WinResult<(IMFTransform, String, bool)> {
    unsafe {
        let entrada = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let saida = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };

        // Hardware primeiro; se a máquina não tiver, cai no de software, que
        // funciona e é lento. Avisar qual entrou é obrigação de quem chama.
        for (so_hardware, bandeiras) in [
            (true, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER),
            (false, MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER),
        ] {
            let mut lista: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut quantos = 0u32;
            if MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                bandeiras,
                Some(&entrada),
                Some(&saida),
                &mut lista,
                &mut quantos,
            )
            .is_err()
                || quantos == 0
            {
                continue;
            }

            let encontrados = std::slice::from_raw_parts(lista, quantos as usize);
            for ativacao in encontrados.iter().flatten() {
                if let Ok(mft) = ativacao.ActivateObject::<IMFTransform>() {
                    let nome = nome_de(ativacao);
                    windows::Win32::System::Com::CoTaskMemFree(Some(lista as *const _));
                    return Ok((mft, nome, so_hardware));
                }
            }
            windows::Win32::System::Com::CoTaskMemFree(Some(lista as *const _));
        }
        Err(windows::core::Error::new(
            windows::Win32::Foundation::E_FAIL,
            "nenhum codificador de H.264 nesta máquina",
        ))
    }
}

fn nome_de(ativacao: &IMFActivate) -> String {
    unsafe {
        let mut ponteiro = windows::core::PWSTR::null();
        let mut tamanho = 0u32;
        if ativacao
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut ponteiro, &mut tamanho)
            .is_err()
        {
            return "sem nome".into();
        }
        let texto = ponteiro.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(ponteiro.0 as *const _));
        texto
    }
}

fn juntar(alto: u32, baixo: u32) -> u64 {
    ((alto as u64) << 32) | baixo as u64
}

/// Só para o compilador não reclamar do GUID importado sem uso direto.
const _: GUID = MFVideoFormat_NV12;
