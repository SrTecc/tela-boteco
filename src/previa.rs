//! A prévia: um quadro pequeno do que está indo ao ar.
//!
//! É a resposta para o pior defeito possível deste app, que já aconteceu de
//! verdade: transmissão com **zero quadro**, sem erro nenhum, e do outro lado
//! só tela preta. Contador diz que saíram pacotes; só a imagem diz que saiu a
//! imagem certa.
//!
//! Três cuidados, todos pelo mesmo motivo — quem transmite está jogando, e a
//! prévia não pode cobrar caro:
//!
//! 1. **A redução é na placa.** O mesmo processador de vídeo que já converte
//!    BGRA em NV12 faz uma segunda saída BGRA pequena. Ler 1080p da placa
//!    quatro vezes por segundo seria 32 MB/s atravessando o barramento para
//!    desenhar uma miniatura.
//! 2. **Quatro quadros por segundo.** Prévia não é monitor: serve para saber
//!    que a imagem está indo e qual é, e isso quatro vezes por segundo já diz.
//! 3. **Um quadro só esperando**, nunca uma fila. Ao vivo, quadro velho não
//!    interessa a ninguém.

use windows::core::{Interface, Result as WinResult};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Altura da miniatura. O bloco da janela tem uns 430 pixels de largura, então
/// 180 de altura cobre o 16:9 inteiro sem sobrar imagem que ninguém vê.
pub const ALTURA: u32 = 180;

/// De quanto em quanto tempo tirar uma. Ver nota 2 no topo.
pub const INTERVALO: std::time::Duration = std::time::Duration::from_millis(250);

pub struct Miniatura {
    video: ID3D11VideoContext,
    processador: ID3D11VideoProcessor,
    enumerador: ID3D11VideoProcessorEnumerator,
    d3d: ID3D11Device,
    contexto: ID3D11DeviceContext,
    /// Onde a placa desenha a versão pequena.
    destino: ID3D11Texture2D,
    /// A mesma coisa, mas numa memória que a CPU consegue ler.
    copia: ID3D11Texture2D,
    saida: Option<ID3D11VideoProcessorOutputView>,
    pub largura: u32,
    pub altura: u32,
}

impl Miniatura {
    pub fn nova(
        d3d: &ID3D11Device,
        contexto: &ID3D11DeviceContext,
        larg_entra: u32,
        alt_entra: u32,
    ) -> WinResult<Self> {
        // A mesma conta do preset de qualidade: mantém a proporção, não
        // aumenta o que já é menor, e devolve lado par.
        let (largura, altura) = crate::qualidade::dimensoes(larg_entra, alt_entra, ALTURA);
        unsafe {
            let video_dev: ID3D11VideoDevice = d3d.cast()?;
            let video: ID3D11VideoContext = contexto.cast()?;

            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: larg_entra,
                InputHeight: alt_entra,
                OutputWidth: largura,
                OutputHeight: altura,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };
            let enumerador = video_dev.CreateVideoProcessorEnumerator(&desc)?;
            let processador = video_dev.CreateVideoProcessor(&enumerador, 0)?;

            let mut desc_destino = D3D11_TEXTURE2D_DESC {
                Width: largura,
                Height: altura,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..Default::default()
            };
            let mut destino: Option<ID3D11Texture2D> = None;
            d3d.CreateTexture2D(&mut desc_destino, None, Some(&mut destino))?;

            // A textura de leitura. Sem ela não há como a CPU ver o que a
            // placa desenhou: textura de desenho não se lê daqui.
            let mut desc_copia = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..desc_destino
            };
            let mut copia: Option<ID3D11Texture2D> = None;
            d3d.CreateTexture2D(&mut desc_copia, None, Some(&mut copia))?;

            Ok(Self {
                video,
                processador,
                enumerador,
                d3d: d3d.clone(),
                contexto: contexto.clone(),
                destino: destino.unwrap(),
                copia: copia.unwrap(),
                saida: None,
                largura,
                altura,
            })
        }
    }

    /// Tira a miniatura do quadro que acabou de chegar.
    pub fn tirar(&mut self, origem: &ID3D11Texture2D) -> WinResult<crate::painel::Quadro> {
        unsafe {
            let video_dev: ID3D11VideoDevice = self.d3d.cast()?;

            if self.saida.is_none() {
                let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };
                let mut v: Option<ID3D11VideoProcessorOutputView> = None;
                video_dev.CreateVideoProcessorOutputView(
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
            video_dev.CreateVideoProcessorInputView(
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
            self.video.VideoProcessorBlt(
                &self.processador,
                self.saida.as_ref().unwrap(),
                0,
                &[fluxo],
            )?;

            self.contexto.CopyResource(&self.copia, &self.destino);

            // Ler daqui para a CPU para a placa até a cópia ficar pronta. É o
            // preço da prévia, e é por isso que ela sai quatro vezes por
            // segundo e num tamanho pequeno, e não a cada quadro.
            let mut mapeado = D3D11_MAPPED_SUBRESOURCE::default();
            self.contexto
                .Map(&self.copia, 0, D3D11_MAP_READ, 0, Some(&mut mapeado))?;
            let rgba = copiar_linhas(
                mapeado.pData as *const u8,
                mapeado.RowPitch as usize,
                self.largura,
                self.altura,
            );
            self.contexto.Unmap(&self.copia, 0);

            Ok(crate::painel::Quadro {
                largura: self.largura,
                altura: self.altura,
                rgba,
            })
        }
    }
}

/// Copia a imagem lida da placa, trocando BGRA por RGBA e tirando a sobra do
/// fim de cada linha.
///
/// A sobra existe sempre: a placa alinha as linhas no tamanho que lhe convém
/// (o `RowPitch`), e ignorar isso é o jeito clássico de a imagem sair
/// enviesada, cada linha deslocada um pouco mais que a anterior.
///
/// # Safety
/// `dados` precisa apontar para `altura * passo` bytes legíveis.
unsafe fn copiar_linhas(dados: *const u8, passo: usize, largura: u32, altura: u32) -> Vec<u8> {
    let mut saida = Vec::with_capacity((largura * altura * 4) as usize);
    for y in 0..altura as usize {
        let linha = std::slice::from_raw_parts(dados.add(y * passo), largura as usize * 4);
        for pixel in linha.chunks_exact(4) {
            saida.push(pixel[2]);
            saida.push(pixel[1]);
            saida.push(pixel[0]);
            saida.push(255);
        }
    }
    saida
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn a_copia_troca_as_cores_e_pula_a_sobra_da_linha() {
        // Duas linhas de 2 pixels, com 8 bytes de sobra no fim de cada uma.
        // Azul puro em BGRA é 255,0,0; em RGBA tem de sair 0,0,255.
        let passo = 2 * 4 + 8;
        let mut bruto = vec![0u8; passo * 2];
        for y in 0..2 {
            for x in 0..2 {
                let i = y * passo + x * 4;
                bruto[i] = 255; // B
                bruto[i + 1] = 0; // G
                bruto[i + 2] = (y * 2 + x) as u8; // R, para distinguir os pixels
                bruto[i + 3] = 0; // A, que a captura nem sempre preenche
            }
        }
        // Lixo na sobra: se ele aparecer na saída, o passo foi ignorado.
        for y in 0..2 {
            for i in 8..passo {
                bruto[y * passo + i] = 99;
            }
        }

        let saida = unsafe { copiar_linhas(bruto.as_ptr(), passo, 2, 2) };
        assert_eq!(saida.len(), 2 * 2 * 4);
        assert!(!saida.contains(&99), "a sobra da linha vazou para a imagem");
        assert_eq!(&saida[0..4], &[0, 0, 255, 255], "o primeiro pixel");
        assert_eq!(&saida[4..8], &[1, 0, 255, 255]);
        assert_eq!(&saida[8..12], &[2, 0, 255, 255], "primeiro pixel da segunda linha");
    }

    #[test]
    fn a_transparencia_sai_sempre_cheia() {
        // A captura do Windows entrega alfa zero em muita janela, e respeitar
        // isso faria a prévia aparecer vazia na tela.
        let bruto = vec![0u8; 4];
        let saida = unsafe { copiar_linhas(bruto.as_ptr(), 4, 1, 1) };
        assert_eq!(saida[3], 255);
    }
}
