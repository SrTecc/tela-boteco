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
use std::sync::Arc;
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
    /// Brilho médio do último quadro, de 0 a 255. Serve para uma coisa só:
    /// provar que veio imagem de verdade. Captura que falha costuma entregar
    /// quadros pretos sem dar erro nenhum, e um contador feliz esconderia isso.
    pub brilho: f64,
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
pub fn capturar_janela(hwnd: isize, duracao_seg: f64) -> WinResult<Resultado> {
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

    let contador = Arc::new(AtomicU64::new(0));
    let ultimo: Arc<std::sync::Mutex<Option<ID3D11Texture2D>>> =
        Arc::new(std::sync::Mutex::new(None));

    {
        let contador = contador.clone();
        let ultimo = ultimo.clone();
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

    let brilho = match ultimo.lock().unwrap().take() {
        Some(textura) => medir_brilho(&d3d, &contexto, &textura)?,
        None => 0.0,
    };

    Ok(Resultado {
        quadros: contador.load(Ordering::Relaxed),
        segundos,
        largura: tamanho.Width,
        altura: tamanho.Height,
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
