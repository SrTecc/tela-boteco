//! Captura do som de **um processo**, e não do sistema inteiro.
//!
//! É a peça que justifica o app existir. O navegador oferece "áudio da aba" ou
//! "áudio do sistema", e nada entre os dois: sistema inteiro inclui o Discord,
//! então quem assiste ouve a própria voz de volta com atraso. Aqui o som sai do
//! processo escolhido, e a voz da call simplesmente não entra.
//!
//! O caminho é o `ActivateAudioInterfaceAsync` com parâmetros de loopback por
//! processo. Duas coisas o tornam incomum:
//!
//! - Não existe "dispositivo" para abrir. Passa-se um nome fixo de dispositivo
//!   virtual e, junto, a ficha dizendo qual processo ouvir.
//! - A ativação é **assíncrona** e responde por um callback COM, que precisa
//!   ser implementado de verdade. Daí o `#[implement]` mais abaixo: é o mínimo
//!   de COM que dá para escrever aqui.
//!
//! O formato é fixado em 48 kHz, estéreo, float: loopback por processo não tem
//! formato nativo para consultar, diferente de um dispositivo de verdade.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use windows::core::{implement, Interface, Result as WinResult, PCWSTR};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Variant::VT_BLOB;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, SetEvent, INFINITE};

pub const TAXA: u32 = 48_000;
pub const CANAIS: u16 = 2;

/// Um pedaço de som capturado, com o instante em que foi gravado.
pub struct Pedaco {
    /// Amostras intercaladas (esquerda, direita, esquerda...), em float.
    pub amostras: Vec<f32>,
    /// Instante no mesmo relógio que a captura de vídeo usa (100ns).
    pub tempo: i64,
}

/// O que ouvir: só o processo escolhido, ou tudo menos ele.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Modo {
    /// O som daquele processo e dos filhos dele. É o caso do jogo escolhido.
    Incluir,
    /// Tudo menos aquele processo. Serve para "a tela inteira, sem o Discord",
    /// quando a pessoa transmite um monitor e não uma janela.
    Excluir,
}

/// Quem avisa que a ativação assíncrona terminou.
///
/// Existe só para destravar a espera: o Windows chama isto de outra thread
/// quando o dispositivo virtual está pronto.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Aviso {
    evento: HANDLE,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Aviso_Impl {
    fn ActivateCompleted(
        &self,
        _operacao: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> WinResult<()> {
        unsafe {
            let _ = SetEvent(self.evento);
        }
        Ok(())
    }
}

/// Embrulho para levar os objetos COM até a thread que puxa o som.
///
/// Seguro por construção: eles são criados aqui, entregues a UMA thread, e
/// ninguém mais os toca. O que não se pode é dois donos ao mesmo tempo, e
/// isso o  do closure já garante.
struct Atravessa {
    cliente: IAudioClient,
    capturador: IAudioCaptureClient,
}
unsafe impl Send for Atravessa {}

pub struct CapturaAudio {
    pub receptor: Receiver<Pedaco>,
    parar: Arc<std::sync::atomic::AtomicBool>,
}

impl CapturaAudio {
    /// Abre a captura e começa a entregar pedaços numa thread própria.
    pub fn iniciar(pid: u32, modo: Modo) -> WinResult<Self> {
        unsafe {
            // Multithreaded porque o aviso de ativação chega noutra thread. O
            // erro é ignorado de propósito: COM já inicializado noutro modo
            // devolve falha, e isso não é problema nosso.
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let cliente = abrir_cliente(pid, modo)?;
        let formato = formato();

        unsafe {
            cliente.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                // Um quinto de segundo de folga: suficiente para o sistema não
                // perder som se esta thread demorar, e pouco para não atrasar.
                2_000_000,
                0,
                &formato,
                None,
            )?;
        }

        let capturador: IAudioCaptureClient = unsafe { cliente.GetService()? };
        unsafe { cliente.Start()? };

        let (envio, receptor) = channel();
        let parar = Arc::new(std::sync::atomic::AtomicBool::new(false));

        {
            let parar = parar.clone();
            let pecas = Atravessa { cliente, capturador };
            std::thread::spawn(move || {
                let pecas = pecas;
                laco(&pecas.capturador, &envio, &parar);
                unsafe {
                    let _ = pecas.cliente.Stop();
                }
            });
        }

        Ok(Self { receptor, parar })
    }

    pub fn parar(&self) {
        self.parar.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// O laço que puxa o som. Roda numa thread só dele.
fn laco(
    capturador: &IAudioCaptureClient,
    envio: &Sender<Pedaco>,
    parar: &Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    while !parar.load(Ordering::Relaxed) {
        // Sem pacote agora não é erro: é silêncio. Dorme um pouco e volta.
        unsafe {
            let disponivel = capturador.GetNextPacketSize().unwrap_or(0);
            if disponivel == 0 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }

            let mut dados: *mut u8 = std::ptr::null_mut();
            let mut quadros = 0u32;
            let mut bandeiras = 0u32;
            let mut posicao_qpc = 0u64;

            if capturador
                .GetBuffer(
                    &mut dados,
                    &mut quadros,
                    &mut bandeiras,
                    None,
                    Some(&mut posicao_qpc),
                )
                .is_err()
            {
                continue;
            }

            let total = quadros as usize * CANAIS as usize;
            // O Windows avisa quando o trecho é silêncio em vez de mandar
            // zeros: aceitar a dica custa menos que copiar silêncio.
            let amostras = if bandeiras & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                vec![0.0f32; total]
            } else {
                std::slice::from_raw_parts(dados as *const f32, total).to_vec()
            };

            let _ = envio.send(Pedaco {
                amostras,
                // A posição vem no mesmo relógio (100ns) que o vídeo usa, e é
                // isso que permite juntar os dois sem um ficar adiantado.
                tempo: posicao_qpc as i64,
            });

            let _ = capturador.ReleaseBuffer(quadros);
        }
    }
}

/// Abre o cliente de áudio do processo escolhido.
fn abrir_cliente(pid: u32, modo: Modo) -> WinResult<IAudioClient> {
    unsafe {
        let mut ficha = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: match modo {
                        Modo::Incluir => PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                        Modo::Excluir => PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            },
        };

        // A ficha viaja dentro de um PROPVARIANT como bloco de bytes: é o
        // jeito que esta API pede, por mais estranho que pareça.
        //
        // Montada à mão, e não pelo tipo da biblioteca, porque aquele tem
        // Drop: ao sair de escopo ele tentaria LIBERAR este bloco, que vive
        // na pilha. Passar o ponteiro cru evita que alguém libere memória
        // que não é dele.
        let propriedade = PropVariantCru {
            tipo: VT_BLOB.0,
            _reservado: [0; 3],
            blob: BLOB {
                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: &mut ficha as *mut _ as *mut u8,
            },
        };
        let propriedade = &propriedade as *const PropVariantCru
            as *const windows::core::PROPVARIANT;

        let evento = CreateEventW(None, false, false, PCWSTR::null())?;
        let aviso: IActivateAudioInterfaceCompletionHandler = Aviso { evento }.into();

        let operacao: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(propriedade),
            &aviso,
        )?;

        if WaitForSingleObject(evento, INFINITE) != WAIT_OBJECT_0 {
            return Err(windows::core::Error::from_win32());
        }

        let mut resultado = windows::core::HRESULT(0);
        let mut interface: Option<windows::core::IUnknown> = None;
        operacao.GetActivateResult(&mut resultado, &mut interface)?;
        resultado.ok()?;

        interface
            .ok_or_else(windows::core::Error::from_win32)?
            .cast::<IAudioClient>()
    }
}

/// O formato pedido. Loopback por processo não tem formato nativo para
/// consultar, então quem escolhe somos nós.
fn formato() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
        nChannels: CANAIS,
        nSamplesPerSec: TAXA,
        nAvgBytesPerSec: TAXA * CANAIS as u32 * 4,
        nBlockAlign: CANAIS * 4,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

/// O PROPVARIANT como ele é na memória, para poder escrever um BLOB nele.
/// A versão amigável da crate não expõe esse campo.
#[repr(C)]
struct PropVariantCru {
    tipo: u16,
    _reservado: [u16; 3],
    blob: BLOB,
}

