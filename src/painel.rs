//! O que a interface mostra e a transmissão escreve.
//!
//! A interface desenha 60 vezes por segundo e **não pode esperar por nada**:
//! uma leitura de rede na thread do desenho trava a janela, e janela travada é
//! o que faz um aplicativo parecer quebrado quando ele está só ocupado. Então
//! a transmissão vive numa thread própria e os dois lados conversam por aqui:
//! números em contadores atômicos, que se leem sem travar ninguém.
//!
//! O sentido contrário é só um: o botão de parar. A transmissão não é
//! interrompida de fora (abortar no meio deixaria a tela presa no site), ela
//! **pede licença** olhando este sinal e desliga arrumando a casa.
//!
//! As funções de formatar são puras de propósito. São elas que decidem o que a
//! pessoa lê, e o que a pessoa lê é justamente o que não dá para conferir
//! olhando um teste de rede.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Painel {
    parar: AtomicBool,
    /// Ligado quando o site DEU a tela, e não quando o botão foi clicado. A
    /// interface precisa dos dois momentos: entre um e outro ela diz "içando",
    /// porque pedir a tela pode acabar em "outra pessoa já está no ar".
    no_ar: AtomicBool,
    pacotes: AtomicU64,
    bytes: AtomicU64,
    espectadores: AtomicUsize,
    pessoas: AtomicUsize,
    /// A última linha de situação. Vai para a janela, não para o terminal.
    recado: Mutex<String>,
    espelho: Arc<Espelho>,
}

impl Painel {
    pub fn novo() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// O botão de tirar do ar. Não mata nada: avisa.
    pub fn pedir_para_parar(&self) {
        self.parar.store(true, Ordering::Relaxed);
    }

    pub fn pediram_para_parar(&self) -> bool {
        self.parar.load(Ordering::Relaxed)
    }

    /// O site deu a tela: daqui em diante é transmissão de verdade.
    pub fn marcar_no_ar(&self) {
        self.no_ar.store(true, Ordering::Relaxed);
    }

    pub fn esta_no_ar(&self) -> bool {
        self.no_ar.load(Ordering::Relaxed)
    }

    pub fn anotar_pacote(&self, bytes: usize) {
        self.pacotes.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn pacotes(&self) -> u64 {
        self.pacotes.load(Ordering::Relaxed)
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Quanta gente esta na call, pela batida da propria transmissao: enquanto
    /// ela roda, a batida de fora fica parada e o numero dela envelhece.
    pub fn anotar_pessoas(&self, quantas: usize) {
        self.pessoas.store(quantas, Ordering::Relaxed);
    }

    pub fn pessoas(&self) -> usize {
        self.pessoas.load(Ordering::Relaxed)
    }

    pub fn anotar_espectadores(&self, quantos: usize) {
        self.espectadores.store(quantos, Ordering::Relaxed);
    }

    pub fn espectadores(&self) -> usize {
        self.espectadores.load(Ordering::Relaxed)
    }

    pub fn dizer(&self, recado: impl Into<String>) {
        if let Ok(mut r) = self.recado.lock() {
            *r = recado.into();
        }
    }

    /// O espelho onde a captura pousa a miniatura do que esta indo ao ar.
    pub fn espelho(&self) -> Arc<Espelho> {
        Arc::clone(&self.espelho)
    }

    pub fn recado(&self) -> String {
        self.recado.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

/// Um quadro pequeno do que está indo ao ar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quadro {
    pub largura: u32,
    pub altura: u32,
    /// RGBA, linha a linha e sem sobra no fim da linha.
    pub rgba: Vec<u8>,
}

/// Onde o último quadro pousa, para a janela pegar.
///
/// **Um só, e não uma fila.** É a mesma regra do codificador: ao vivo, quadro
/// velho não interessa a ninguém, e enfileirar só faria a prévia mostrar o
/// passado com atraso crescente.
#[derive(Default)]
pub struct Espelho {
    quadro: Mutex<Option<Quadro>>,
    /// Se há janela olhando. Fechada para a bandeja, ninguém olha, e aí tirar
    /// miniatura quatro vezes por segundo seria gastar a placa de quem está
    /// jogando para desenhar o que ninguém vê — justamente o contrário do que
    /// a bandeja existe para fazer.
    olhando: AtomicBool,
    /// Sobe a cada quadro novo. A janela desenha 60 vezes por segundo e a
    /// prévia muda 4: sem isto, ela mandaria a mesma imagem para a placa
    /// quinze vezes à toa.
    versao: AtomicU64,
}

impl Espelho {
    pub fn novo() -> Arc<Self> {
        let espelho = Self::default();
        // Começa ligado: quem não sabe desta conversa (a linha de comando) tem
        // de receber a miniatura do mesmo jeito.
        espelho.olhando.store(true, Ordering::Relaxed);
        Arc::new(espelho)
    }

    /// A janela avisa se está olhando.
    pub fn olhando(&self, sim: bool) {
        self.olhando.store(sim, Ordering::Relaxed);
    }

    /// Vale a pena tirar uma miniatura agora?
    pub fn quer_quadro(&self) -> bool {
        self.olhando.load(Ordering::Relaxed)
    }

    pub fn pousar(&self, quadro: Quadro) {
        if let Ok(mut lugar) = self.quadro.lock() {
            *lugar = Some(quadro);
            self.versao.fetch_add(1, Ordering::Release);
        }
    }

    /// O quadro, só se for mais novo que o que quem pergunta já tem.
    pub fn pegar_se_novo(&self, ultima: &mut u64) -> Option<Quadro> {
        let agora = self.versao.load(Ordering::Acquire);
        if agora == *ultima {
            return None;
        }
        *ultima = agora;
        self.quadro.lock().ok().and_then(|q| q.clone())
    }
}

/// O tempo no ar, como um relógio.
///
/// Minuto e segundo abaixo de uma hora: ninguém lê "00:03:12" mais rápido que
/// "03:12", e transmissão de boteco dura minutos.
pub fn relogio(segundos: u64) -> String {
    let (h, m, s) = (segundos / 3600, (segundos % 3600) / 60, segundos % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Quanto já subiu. Vírgula, não ponto: a casa fala português.
pub fn tamanho(bytes: u64) -> String {
    const MB: f64 = 1_048_576.0;
    if bytes < 1_048_576 {
        format!("{} kB", bytes / 1024)
    } else {
        format!("{:.2} MB", bytes as f64 / MB).replace('.', ",")
    }
}

/// O upload, que é O número desta história: quem transmite envia o vídeo uma
/// vez **por espectador**, então é ele que estoura antes de qualquer outra
/// coisa. Em megabits, que é a unidade em que todo plano de internet é vendido.
pub fn vazao(bytes: u64, segundos: f64) -> String {
    if segundos <= 0.0 {
        return "—".to_string();
    }
    let mbit = (bytes as f64 * 8.0) / segundos / 1_000_000.0;
    format!("{mbit:.1} Mb/s").replace('.', ",")
}

/// Quem está assistindo, em português e sem "1 espectador(es)".
pub fn frase_espectadores(quantos: usize) -> String {
    match quantos {
        0 => "ninguém assistindo ainda".to_string(),
        1 => "1 pessoa assistindo".to_string(),
        n => format!("{n} pessoas assistindo"),
    }
}

/// O aviso de tela parada, que existe por causa de um estrago real.
///
/// A captura do Windows só entrega quadro quando a imagem **muda**, e janela
/// que fica atrás das outras às vezes para de desenhar. O resultado é uma
/// transmissão sem erro nenhum, com zero quadro, e do outro lado só tela
/// preta. Aconteceu com gente esperando, e o relatório que diria "0 quadros"
/// só apareceu quando acabou.
///
/// Seis segundos porque abaixo disso o começo normal (abrir o codificador,
/// fechar a primeira ligação) acusaria sozinho.
pub fn aviso_de_tela_parada(pacotes: u64, segundos: f64) -> Option<&'static str> {
    (pacotes == 0 && segundos >= 6.0).then_some(
        "Nenhum quadro saiu ainda. A janela escolhida não está desenhando \
         (minimizada, ou parada por estar atrás das outras). Traz ela para a frente.",
    )
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn o_relogio_omite_a_hora_que_nao_passou() {
        assert_eq!(relogio(0), "00:00");
        assert_eq!(relogio(9), "00:09");
        assert_eq!(relogio(192), "03:12");
        assert_eq!(relogio(3599), "59:59");
        assert_eq!(relogio(3600), "1:00:00");
        assert_eq!(relogio(3792), "1:03:12");
    }

    #[test]
    fn o_tamanho_sai_com_virgula() {
        // Ponto decimal em texto português é descuido que aparece na tela.
        assert_eq!(tamanho(0), "0 kB");
        assert_eq!(tamanho(2048), "2 kB");
        assert_eq!(tamanho(6_312_755), "6,02 MB");
    }

    #[test]
    fn a_vazao_e_em_megabits_e_aguenta_tempo_zero() {
        // Tempo zero acontece no primeiro desenho, antes de qualquer pacote:
        // dividir ali daria infinito na tela.
        assert_eq!(vazao(0, 0.0), "—");
        assert_eq!(vazao(1_000_000, 0.0), "—");
        // 1 MB em 8 segundos = 1 Mb/s.
        assert_eq!(vazao(1_000_000, 8.0), "1,0 Mb/s");
        assert_eq!(vazao(5_000_000, 2.0), "20,0 Mb/s");
    }

    #[test]
    fn espectador_no_singular_nao_leva_s() {
        assert_eq!(frase_espectadores(0), "ninguém assistindo ainda");
        assert_eq!(frase_espectadores(1), "1 pessoa assistindo");
        assert_eq!(frase_espectadores(4), "4 pessoas assistindo");
    }

    #[test]
    fn o_aviso_de_tela_parada_espera_os_seis_segundos() {
        // Antes disso, zero pacote é só o começo normal.
        assert!(aviso_de_tela_parada(0, 0.0).is_none());
        assert!(aviso_de_tela_parada(0, 5.9).is_none());
        assert!(aviso_de_tela_parada(0, 6.0).is_some());
    }

    #[test]
    fn pacote_que_saiu_cala_o_aviso() {
        // E cala para sempre: a tela pode parar depois, e o que o aviso
        // responde é "isto nunca funcionou", que é outra pergunta.
        assert!(aviso_de_tela_parada(1, 600.0).is_none());
    }

    fn quadro(marca: u8) -> Quadro {
        Quadro { largura: 2, altura: 1, rgba: vec![marca; 8] }
    }

    #[test]
    fn o_espelho_entrega_o_quadro_uma_vez_so() {
        // Mandar a mesma imagem para a placa a cada desenho seria quinze vezes
        // o trabalho, para a mesma imagem.
        let e = Espelho::novo();
        let mut vista = 0;
        assert_eq!(e.pegar_se_novo(&mut vista), None, "espelho vazio nao devolve nada");
        e.pousar(quadro(7));
        assert_eq!(e.pegar_se_novo(&mut vista), Some(quadro(7)));
        assert_eq!(e.pegar_se_novo(&mut vista), None, "o mesmo quadro nao volta");
    }

    #[test]
    fn com_a_janela_escondida_nem_vale_tirar_a_miniatura() {
        let e = Espelho::novo();
        assert!(e.quer_quadro(), "sem ninguem dizer nada, tira");
        e.olhando(false);
        assert!(!e.quer_quadro());
        e.olhando(true);
        assert!(e.quer_quadro());
    }

    #[test]
    fn quadro_velho_nao_fila_atras_do_novo() {
        // Ao vivo, quadro velho nao interessa a ninguem: a janela tem de
        // pegar o ULTIMO, e nao o primeiro que chegou sem ser lido.
        let e = Espelho::novo();
        let mut vista = 0;
        e.pousar(quadro(1));
        e.pousar(quadro(2));
        assert_eq!(e.pegar_se_novo(&mut vista), Some(quadro(2)));
        assert_eq!(e.pegar_se_novo(&mut vista), None);
    }

    #[test]
    fn o_pedido_de_parar_e_so_um_aviso() {
        let p = Painel::novo();
        assert!(!p.pediram_para_parar());
        p.pedir_para_parar();
        assert!(p.pediram_para_parar());
    }

    #[test]
    fn so_esta_no_ar_depois_que_o_site_deu_a_tela() {
        // Clicar no botão não é estar no ar: o pedido pode ser recusado porque
        // outra pessoa chegou antes, e a trava fica no site.
        let p = Painel::novo();
        assert!(!p.esta_no_ar());
        p.marcar_no_ar();
        assert!(p.esta_no_ar());
    }

    #[test]
    fn os_contadores_somam_pacote_e_tamanho_juntos() {
        let p = Painel::novo();
        p.anotar_pacote(1200);
        p.anotar_pacote(800);
        assert_eq!(p.pacotes(), 2);
        assert_eq!(p.bytes(), 2000);
    }
}
