//! Captura de uma janela pelo Windows Graphics Capture.
//!
//! É a peça mais arriscada do app, e por isso a primeira a ser construída. O
//! que importa aqui não é "conseguir um quadro": é conseguir o quadro **como
//! textura na placa de vídeo**, sem nenhuma volta pela CPU. É essa diferença
//! que separa um app leve de um que faz o jogo engasgar, e foi exatamente onde
//! o ffmpeg falhou nesta máquina durante os testes.
//!
//! Dois detalhes que custam tempo de quem não os conhece:
//!
//! - O `FramePool` é criado **livre de thread** (`CreateFreeThreaded`). Com a
//!   versão comum, o aviso de quadro pronto só chega para quem tem laço de
//!   mensagens do Windows rodando, e um app sem janela aberta não tem.
//! - Captura é **assíncrona**: o Windows avisa quando quer, não quando se
//!   pede. Então o que existe aqui é um contador e um canal, e não um laço
//!   pedindo quadros.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::{Interface, Result as WinResult};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

/// O que uma sessão de captura produziu.
pub struct Resultado {
    pub quadros: u64,
    pub segundos: f64,
    pub largura: i32,
    pub altura: i32,
    /// O que o gravador apurou, quando houve gravação.
    pub gravacao: Option<Gravacao>,
    /// Brilho médio do último quadro, de 0 a 255. Serve para uma coisa só:
    /// provar que veio imagem de verdade. Captura que falha costuma entregar
    /// quadros pretos sem dar erro nenhum, e um contador feliz esconderia isso.
    pub brilho: f64,
}

/// O que a gravação produziu. Existe separado porque a pergunta que ela
/// responde é outra: a captura pergunta "chega imagem?", e a gravação
/// pergunta "quem codificou, o chip ou a CPU?".
#[derive(Debug, Clone)]
pub struct Gravacao {
    pub quadros: u64,
    pub em_hardware: bool,
    pub codificador: String,
    pub arquivo: String,
}

impl Resultado {
    pub fn fps(&self) -> f64 {
        if self.segundos > 0.0 {
            self.quadros as f64 / self.segundos
        } else {
            0.0
        }
    }
}

/// Captura uma janela por `duracao_seg` segundos e devolve o que aconteceu.
///
/// Com `gravador`, cada quadro vai também para o codificador. O gravador vem
/// embrulhado porque o aviso de quadro pronto chega numa thread do sistema, e
/// não nesta: é o Windows que decide quando entregar.
pub fn capturar_janela(
    hwnd: isize,
    duracao_seg: f64,
    gravar_em: Option<&str>,
    bitrate: u32,
) -> WinResult<Resultado> {
    if !GraphicsCaptureSession::IsSupported()? {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_NOTIMPL,
            "este Windows não tem Graphics Capture",
        ));
    }

    let (d3d, contexto) = criar_dispositivo()?;
    let dispositivo_winrt = unsafe {
        let dxgi: IDXGIDevice = d3d.cast()?;
        CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?
    };
    let dispositivo_winrt: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice =
        dispositivo_winrt.cast()?;

    // A ponte entre o HWND (mundo Win32) e o item de captura (mundo WinRT).
    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(HWND(hwnd as *mut _))? };

    let tamanho = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &dispositivo_winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        // Duas casas de quadro: uma sendo preenchida e outra sendo lida. Mais
        // que isso só adiciona atraso, que num app ao vivo é o que não se quer.
        2,
        tamanho,
    )?;

    // O gravador nasce aqui porque precisa da MESMA placa da captura: textura
    // de um dispositivo não atravessa para outro. O tamanho vem arredondado
    // para baixo em número par, exigência do H.264.
    let gravador = match gravar_em {
        Some(caminho) => Some(Arc::new(Mutex::new(crate::codificador::Gravador::novo(
            &d3d,
            &contexto,
            caminho,
            (tamanho.Width as u32) & !1,
            (tamanho.Height as u32) & !1,
            60,
            bitrate,
        )?))),
        None => None,
    };

    let contador = Arc::new(AtomicU64::new(0));
    let ultimo: Arc<Mutex<Option<ID3D11Texture2D>>> = Arc::new(Mutex::new(None));

    {
        let contador = contador.clone();
        let ultimo = ultimo.clone();
        let gravador = gravador.clone();
        pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, _>::new(
            move |pool: &Option<Direct3D11CaptureFramePool>, _| {
                let Some(pool) = pool.as_ref() else {
                    return Ok(());
                };
                // `TryGetNextFrame` devolve None quando não há quadro novo: é o
                // caso de janela parada, e não é erro.
                if let Ok(quadro) = pool.TryGetNextFrame() {
                    if let Ok(superficie) = quadro.Surface() {
                        if let Ok(acesso) = superficie.cast::<IDirect3DDxgiInterfaceAccess>() {
                            if let Ok(textura) = unsafe { acesso.GetInterface::<ID3D11Texture2D>() }
                            {
                                // Gravar aqui, e não numa fila, de propósito:
                                // o codificador é assíncrono por dentro, então
                                // `escrever` devolve antes de a codificação
                                // terminar. Uma fila só adicionaria atraso.
                                if let Some(g) = gravador.as_ref() {
                                    // O instante vem do próprio quadro, e não
                                    // do relógio daqui: entre o Windows
                                    // entregar e esta linha rodar há atraso
                                    // variável, e é ele que faz vídeo gravado
                                    // sair com ritmo torto.
                                    let tempo = quadro
                                        .SystemRelativeTime()
                                        .map(|t| t.Duration)
                                        .unwrap_or(0);
                                    if let Ok(mut g) = g.lock() {
                                        let _ = g.escrever(&textura, tempo);
                                    }
                                }
                                *ultimo.lock().unwrap() = Some(textura);
                            }
                        }
                    }
                    contador.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            },
        ))?;
    }

    let sessao = pool.CreateCaptureSession(&item)?;
    // A borda amarela que o Windows 11 desenha em volta do que está sendo
    // capturado. Desligar exige permissão que app sem pacote não tem, então a
    // falha aqui é esperada e ignorada de propósito.
    let _ = sessao.SetIsBorderRequired(false);
    sessao.StartCapture()?;

    let inicio = Instant::now();
    std::thread::sleep(std::time::Duration::from_secs_f64(duracao_seg));
    let segundos = inicio.elapsed().as_secs_f64();

    sessao.Close()?;
    pool.Close()?;

    // Fecha o gravador ANTES de medir brilho: o Finalize e quem escreve o
    // indice do MP4, e arquivo sem indice nao abre em player nenhum.
    let gravacao = match gravador {
        Some(g) => {
            let g = g.lock().unwrap();
            g.fechar()?;
            Some(Gravacao {
                quadros: g.quadros,
                em_hardware: g.em_hardware,
                codificador: g.codificador.clone(),
                arquivo: gravar_em.unwrap_or_default().to_string(),
            })
        }
        None => None,
    };

    let brilho = match ultimo.lock().unwrap().take() {
        Some(textura) => medir_brilho(&d3d, &contexto, &textura)?,
        None => 0.0,
    };

    Ok(Resultado {
        quadros: contador.load(Ordering::Relaxed),
        segundos,
        largura: tamanho.Width,
        altura: tamanho.Height,
        gravacao,
        brilho,
    })
}

/// Cria o dispositivo Direct3D que vai receber os quadros.
fn criar_dispositivo() -> WinResult<(ID3D11Device, ID3D11DeviceContext)> {
    let mut d3d: Option<ID3D11Device> = None;
    let mut contexto: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            // BGRA é exigência do Graphics Capture, não preferência nossa.
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            Some(&mut contexto),
        )?;
    }
    Ok((d3d.unwrap(), contexto.unwrap()))
}

/// Traz UM quadro para a CPU só para conferir que ele não é preto.
///
/// Isto é instrumento de teste, não caminho de produção: em produção o quadro
/// vai da captura para o codificador sem sair da placa. Fazer esta volta a cada
/// quadro é exatamente o erro que o app existe para não cometer.
fn medir_brilho(
    d3d: &ID3D11Device,
    contexto: &ID3D11DeviceContext,
    textura: &ID3D11Texture2D,
) -> WinResult<f64> {
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        textura.GetDesc(&mut desc);

        let mut copia = desc;
        copia.Usage = D3D11_USAGE_STAGING;
        copia.BindFlags = 0;
        copia.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        copia.MiscFlags = 0;

        let mut staging: Option<ID3D11Texture2D> = None;
        d3d.CreateTexture2D(&copia, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        contexto.CopyResource(&staging, textura);

        let mut mapeado = D3D11_MAPPED_SUBRESOURCE::default();
        contexto.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapeado))?;

        // Amostra um pixel a cada 16, nas duas direções: medir todos custaria
        // tempo à toa para responder "tem imagem?".
        let mut soma = 0u64;
        let mut quantos = 0u64;
        let linha = mapeado.RowPitch as usize;
        let dados = mapeado.pData as *const u8;
        let mut y = 0usize;
        while y < desc.Height as usize {
            let mut x = 0usize;
            while x < desc.Width as usize {
                let p = dados.add(y * linha + x * 4);
                soma += *p as u64 + *p.add(1) as u64 + *p.add(2) as u64;
                quantos += 3;
                x += 16;
            }
            y += 16;
        }
        contexto.Unmap(&staging, 0);

        Ok(if quantos > 0 {
            soma as f64 / quantos as f64
        } else {
            0.0
        })
    }
}
