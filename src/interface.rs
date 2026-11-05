//! A janela: escolher o que transmitir, içar e baixar a tela.
//!
//! Três regras organizam este arquivo, e as três existem porque a interface
//! desenha dezenas de vezes por segundo:
//!
//! 1. **Nada de rede aqui.** Login, batida de presença e transmissão vivem em
//!    threads próprias e mandam recado por canal. Requisição na thread do
//!    desenho trava a janela, e janela travada é o que faz um aplicativo
//!    parecer quebrado quando ele está só esperando.
//! 2. **A escolha é guardada pela IDENTIDADE**, não pela posição na lista. A
//!    lista é remontada a cada atualizada e os títulos de janela mudam sozinhos
//!    (a do Spotify se chama pelo nome da música): guardar o índice faria a
//!    pessoa clicar numa coisa e transmitir outra.
//! 3. **O que der errado aparece NA TELA.** Já custou horas um cliente que
//!    engolia falha em silêncio e parecia funcionando.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::painel::{self, Painel};
use crate::tema;

/// Teto de uma transmissão. Quem manda é o botão de baixar a tela; isto é só
/// para um app esquecido aberto não ficar transmitindo a madrugada inteira.
const TETO_SEGUNDOS: f64 = 12.0 * 3600.0;

/// Onde o app está.
enum Estado {
    /// Sem sessão: ninguém sabe quem é.
    Fora,
    /// Navegador aberto, esperando a volta do Discord.
    Entrando,
    Parado,
    /// Pediu a tela ao site e está montando a transmissão.
    Icando,
    NoAr {
        desde: Instant,
    },
    /// Pediu para parar e está arrumando a casa (soltando a tela no site).
    Baixando,
}

/// O que a janela mostra da sala, já resumido pela thread que perguntou.
struct Resumo {
    nome: String,
    quantos: usize,
    /// Quem está com a tela, se não estiver livre, e se sou eu.
    dono_da_tela: Option<(String, bool)>,
}

/// Recados das threads de fundo para a janela.
enum Recado {
    Sessao(Result<String, String>),
    Sala(Result<Resumo, crate::sala::Erro>),
    Acabou(Result<String, String>),
}

pub struct Aplicativo {
    estado: Estado,
    /// Compartilhada com o batedor de ponto, que precisa dela a cada volta.
    sessao: Arc<Mutex<Option<String>>>,
    fontes: Vec<crate::fontes::Fonte>,
    escolhida: Option<crate::captura::Alvo>,
    qualidade: usize,
    sala: Option<Resumo>,
    aviso: Option<String>,
    relatorio: Option<String>,
    painel: Arc<Painel>,
    no_ar: Arc<AtomicBool>,
    focada: Arc<AtomicBool>,
    recebe: Receiver<Recado>,
    manda: Sender<Recado>,
    ctx: egui::Context,
    /// Última amostra de bytes, para a vazão.
    amostra: (u64, Instant),
    vazao: String,
}

/// Abre a janela. Só volta quando ela fecha.
pub fn abrir() -> eframe::Result {
    let opcoes = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tela do Boteco")
            .with_inner_size([500.0, 700.0])
            .with_min_inner_size([420.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Tela do Boteco",
        opcoes,
        Box::new(|cc| {
            tema::aplicar(&cc.egui_ctx);
            Ok(Box::new(Aplicativo::novo(cc.egui_ctx.clone())))
        }),
    )
}

impl Aplicativo {
    fn novo(ctx: egui::Context) -> Self {
        let (manda, recebe) = std::sync::mpsc::channel();
        let guardada = crate::sessao::guardada();
        let mut app = Self {
            estado: if guardada.is_some() { Estado::Parado } else { Estado::Fora },
            sessao: Arc::new(Mutex::new(guardada)),
            fontes: Vec::new(),
            escolhida: None,
            qualidade: crate::qualidade::PRESETS
                .iter()
                .position(|q| q.id == crate::qualidade::padrao().id)
                .unwrap_or(0),
            sala: None,
            aviso: None,
            relatorio: None,
            painel: Painel::novo(),
            no_ar: Arc::new(AtomicBool::new(false)),
            focada: Arc::new(AtomicBool::new(true)),
            recebe,
            manda,
            ctx,
            amostra: (0, Instant::now()),
            vazao: String::new(),
        };
        app.atualizar_fontes();
        match std::env::var("TELA_CENA") {
            // Cena de prova: nada de rede, para a tela não mudar debaixo da
            // foto.
            Ok(cena) => app.montar_cena(&cena),
            Err(_) => app.ligar_batedor(),
        }
        app
    }

    /// Cenas de prova, para conferir a interface sem depender da vida real.
    ///
    /// Mesma ideia do preview do site, e pelo mesmo motivo: ver a tela "no ar"
    /// de verdade exigiria duas pessoas numa call do servidor, e a tela que
    /// ninguém olha antes é a que sai torta. Liga com `TELA_CENA=no-ar`.
    fn montar_cena(&mut self, cena: &str) {
        if cena == "fora" {
            self.estado = Estado::Fora;
            return;
        }
        if let Ok(mut g) = self.sessao.lock() {
            *g = Some("cena.de-prova".into());
        }
        self.estado = Estado::Parado;
        self.sala = Some(Resumo {
            nome: "balcão".into(),
            quantos: 4,
            dono_da_tela: None,
        });
        match cena {
            "no-ar" | "no-ar-com-erro" => {
                self.no_ar.store(true, Ordering::Relaxed);
                self.painel.marcar_no_ar();
                self.painel.anotar_espectadores(2);
                for _ in 0..812 {
                    self.painel.anotar_pacote(7_700);
                }
                self.painel.anotar_pessoas(4);
                self.vazao = "4,9 Mb/s".into();
                if cena == "no-ar-com-erro" {
                    // A cena que mais importa e a que menos aparece: transmissao
                    // que perde o contato com o site solta a tela em 20 segundos.
                    self.painel
                        .dizer("nao consegui avisar o site que continuo aqui: rede: tempo esgotado");
                }
                self.estado = Estado::NoAr {
                    desde: Instant::now() - Duration::from_secs(192),
                };
            }
            "tomada" => {
                if let Some(sala) = self.sala.as_mut() {
                    sala.dono_da_tela = Some(("Donna".into(), false));
                }
            }
            "fora-da-call" => self.sala = None,
            _ => {}
        }
    }

    /// Remonta a lista do que dá para transmitir.
    fn atualizar_fontes(&mut self) {
        self.fontes = crate::fontes::listar();
        match self.escolhida {
            // A escolhida sumiu (fecharam a janela): limpa, em vez de deixar
            // apontando para um handle que não existe mais.
            Some(alvo) if !self.fontes.iter().any(|f| f.alvo == alvo) => self.escolhida = None,
            Some(_) => {}
            // Primeira vez: já vem com a tela principal marcada, que é a
            // escolha mais comum e a que não falha com jogo em tela cheia.
            None => {
                self.escolhida = self
                    .fontes
                    .iter()
                    .find(|f| f.tela_inteira && f.rotulo.contains("principal"))
                    .or_else(|| self.fontes.iter().find(|f| f.tela_inteira))
                    .map(|f| f.alvo);
            }
        }
    }

    /// A thread que pergunta ao site como está a sala.
    ///
    /// É ela que sabe, antes de qualquer clique, se a pessoa está numa call e
    /// se a tela está livre. Sem isso o app só descobriria no erro.
    fn ligar_batedor(&self) {
        let sessao = Arc::clone(&self.sessao);
        let no_ar = Arc::clone(&self.no_ar);
        let focada = Arc::clone(&self.focada);
        let manda = self.manda.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            // A Sala é refeita só quando a sessão muda: o cliente HTTP guarda
            // conexão, e jogá-lo fora a cada batida pagaria TLS de novo.
            let mut atual: Option<(String, crate::sala::Sala)> = None;
            loop {
                let agora = sessao.lock().ok().and_then(|g| g.clone());
                let transmitindo = no_ar.load(Ordering::Relaxed);

                if let (Some(s), false) = (agora, transmitindo) {
                    if atual.as_ref().map(|(g, _)| g != &s).unwrap_or(true) {
                        atual = crate::sala::Sala::nova(crate::SITE, &s)
                            .ok()
                            .map(|sala| (s.clone(), sala));
                    }
                    if let Some((_, sala)) = atual.as_ref() {
                        // `false`: bate o ponto sem pedir a tela. Pedir aqui
                        // tomaria a tela de quem está no ar só por o app estar
                        // aberto.
                        let resposta = rt.block_on(sala.bater_ponto(false));
                        let _ = manda.send(Recado::Sala(resposta.map(resumir)));
                        ctx.request_repaint();
                    }
                }
                std::thread::sleep(intervalo_da_batida(
                    focada.load(Ordering::Relaxed),
                    transmitindo,
                ));
            }
        });
    }

    fn entrar(&mut self) {
        self.estado = Estado::Entrando;
        self.aviso = None;
        let manda = self.manda.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            // Bloqueia esperando o navegador voltar numa porta local. Por isso
            // mora numa thread: aqui dentro a janela continuaria desenhando.
            let r = crate::sessao::entrar(crate::SITE).map_err(|e| e.to_string());
            let _ = manda.send(Recado::Sessao(r));
            ctx.request_repaint();
        });
    }

    fn icar(&mut self) {
        let Some(alvo) = self.escolhida else { return };
        let Some(fonte) = self.fontes.iter().find(|f| f.alvo == alvo).cloned() else {
            self.aviso = Some("essa fonte não está mais aberta".into());
            return;
        };
        let Some(sessao) = self.sessao.lock().ok().and_then(|g| g.clone()) else {
            self.estado = Estado::Fora;
            return;
        };

        // Painel novo a cada transmissão: contador que começa onde o outro
        // parou é número errado na tela.
        self.painel = Painel::novo();
        self.amostra = (0, Instant::now());
        self.vazao = String::new();
        self.relatorio = None;
        self.aviso = None;
        self.estado = Estado::Icando;
        self.no_ar.store(true, Ordering::Relaxed);

        let qualidade = crate::qualidade::PRESETS[self.qualidade];
        let painel = Arc::clone(&self.painel);
        let manda = self.manda.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt.block_on(crate::transmissao::transmitir_na_sala(
                    fonte.alvo,
                    fonte.som,
                    crate::SITE,
                    &sessao,
                    TETO_SEGUNDOS,
                    qualidade,
                    painel,
                )),
                Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
            };
            let _ = manda.send(Recado::Acabou(r.map_err(|e| e.to_string())));
            ctx.request_repaint();
        });
    }

    fn baixar(&mut self) {
        self.painel.pedir_para_parar();
        self.estado = Estado::Baixando;
    }

    fn receber_recados(&mut self) {
        while let Ok(recado) = self.recebe.try_recv() {
            match recado {
                Recado::Sessao(Ok(s)) => {
                    if let Ok(mut g) = self.sessao.lock() {
                        *g = Some(s);
                    }
                    self.estado = Estado::Parado;
                }
                Recado::Sessao(Err(e)) => {
                    self.estado = Estado::Fora;
                    self.aviso = Some(e);
                }
                Recado::Sala(Ok(r)) => {
                    self.sala = Some(r);
                    self.aviso = None;
                }
                Recado::Sala(Err(crate::sala::Erro::SemSessao)) => {
                    // Sessão vencida: não é erro de rede, é hora de entrar de
                    // novo. Guardar a antiga só faria toda batida falhar.
                    if let Ok(mut g) = self.sessao.lock() {
                        *g = None;
                    }
                    self.sala = None;
                    self.estado = Estado::Fora;
                }
                Recado::Sala(Err(crate::sala::Erro::ForaDaCall)) => {
                    // Não é falha: é estado. A pessoa só não está numa call, e
                    // o lugar disso é o rodapé, não um aviso vermelho.
                    self.sala = None;
                    self.aviso = None;
                }
                Recado::Sala(Err(e)) => self.aviso = Some(e.to_string()),
                Recado::Acabou(r) => {
                    self.no_ar.store(false, Ordering::Relaxed);
                    self.estado = Estado::Parado;
                    match r {
                        Ok(relatorio) => self.relatorio = Some(relatorio),
                        Err(e) => self.aviso = Some(e),
                    }
                }
            }
        }
    }

    /// Mede a vazão de vez em quando, e não a cada desenho: a conta sobre um
    /// intervalo de milissegundos balança demais para ser lida.
    fn medir_vazao(&mut self) {
        let passou = self.amostra.1.elapsed().as_secs_f64();
        if passou >= 1.0 {
            let agora = self.painel.bytes();
            self.vazao = painel::vazao(agora.saturating_sub(self.amostra.0), passou);
            self.amostra = (agora, Instant::now());
        }
    }
}

impl eframe::App for Aplicativo {
    /// O balcão também ONDE NÃO HÁ NADA. Sem isto, a parte da janela que o
    /// conteúdo não alcança sai no cinza padrão, e a emenda aparece.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        tema::BALCAO.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.receber_recados();
        self.focada
            .store(ui.ctx().input(|i| i.focused), Ordering::Relaxed);

        if matches!(self.estado, Estado::Icando) && self.painel.esta_no_ar() {
            self.estado = Estado::NoAr { desde: Instant::now() };
        }
        if matches!(self.estado, Estado::NoAr { .. }) {
            self.medir_vazao();
        }

        egui::Frame::new()
            .fill(tema::BALCAO)
            .inner_margin(14)
            .show(ui, |ui| {
                // A pagina inteira rola. Sem isso, um aviso a mais no rodape
                // empurra os numeros da transmissao para fora da janela, e o
                // que some e justamente o que se foi ler.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.desenhar(ui));
            });

        // Enquanto há número mudando na tela, redesenha sozinha. Parada, a
        // janela não gasta nada: egui só desenha quando acontece algo.
        if matches!(self.estado, Estado::NoAr { .. } | Estado::Icando | Estado::Baixando) {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }
}

impl Aplicativo {
    fn desenhar(&mut self, ui: &mut egui::Ui) {
        self.cabecalho(ui);
        ui.add_space(10.0);

        if matches!(self.estado, Estado::Fora | Estado::Entrando) {
            self.bloco_de_entrada(ui);
            self.rodape(ui);
            return;
        }

        self.bloco_de_fontes(ui);
        ui.add_space(8.0);
        self.bloco_de_qualidade(ui);
        ui.add_space(8.0);
        self.bloco_do_balcao(ui);
        self.rodape(ui);
    }

    fn cabecalho(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            selo(ui);
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("TELA DO BOTECO")
                    .heading()
                    .color(tema::TINTA),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (texto, cor) = match self.estado {
                    Estado::NoAr { .. } => ("NO AR", tema::BRASA),
                    Estado::Icando => ("içando", tema::AMBAR),
                    Estado::Baixando => ("baixando", tema::AMBAR),
                    Estado::Entrando => ("entrando", tema::AMBAR),
                    Estado::Fora => ("fora", tema::TINTA_FRACA),
                    Estado::Parado => ("parado", tema::TINTA_FRACA),
                };
                pastilha(ui, texto, cor);
            });
        });
    }

    fn bloco_de_entrada(&mut self, ui: &mut egui::Ui) {
        tema::bloco().show(ui, |ui| {
            ui.label(tema::rotulo("quem está transmitindo"));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "O site precisa saber quem é você para deixar entrar na sala da call. \
                     Abre no navegador e volta sozinho.",
                )
                .color(tema::TINTA),
            );
            ui.add_space(10.0);
            let entrando = matches!(self.estado, Estado::Entrando);
            let botao = egui::Button::new(
                egui::RichText::new(if entrando {
                    "esperando o navegador…"
                } else {
                    "Entrar com o Discord"
                })
                .color(tema::BALCAO),
            )
            .fill(tema::AMBAR);
            if ui
                .add_enabled(!entrando, botao)
                .clicked()
            {
                self.entrar();
            }
        });
    }

    fn bloco_de_fontes(&mut self, ui: &mut egui::Ui) {
        tema::bloco().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(tema::rotulo("o que transmitir"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("atualizar").clicked() {
                        self.atualizar_fontes();
                    }
                });
            });
            ui.add_space(4.0);

            let mut clicada = None;
            egui::ScrollArea::vertical()
                // Quatro linhas inteiras: meia linha cortada parece defeito,
                // e a barra de rolagem ja diz que ha mais.
                .max_height(194.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for fonte in &self.fontes {
                        let marcada = self.escolhida == Some(fonte.alvo);
                        if linha_de_fonte(ui, fonte, marcada) {
                            clicada = Some(fonte.alvo);
                        }
                    }
                    if self.fontes.is_empty() {
                        ui.label(tema::miudo("nada aberto para transmitir"));
                    }
                });
            if let Some(alvo) = clicada {
                self.escolhida = Some(alvo);
            }
        });
    }

    fn bloco_de_qualidade(&mut self, ui: &mut egui::Ui) {
        tema::bloco().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(tema::rotulo("qualidade"));
                ui.add_space(6.0);
                egui::ComboBox::from_id_salt("qualidade")
                    .selected_text(crate::qualidade::PRESETS[self.qualidade].rotulo)
                    .width(ui.available_width() - 4.0)
                    .show_ui(ui, |ui| {
                        for (i, preset) in crate::qualidade::PRESETS.iter().enumerate() {
                            ui.selectable_value(&mut self.qualidade, i, preset.rotulo);
                        }
                    });
            });
            ui.add_space(2.0);
            ui.label(tema::miudo(self.frase_do_som()));
        });
    }

    /// Quem escolhe o que transmitir escolhe também a fonte do som, como no
    /// Discord. Dizer isso em voz alta evita a surpresa de transmitir a tela
    /// inteira e ouvir a própria voz voltando.
    fn frase_do_som(&self) -> String {
        let Some(fonte) = self
            .escolhida
            .and_then(|alvo| self.fontes.iter().find(|f| f.alvo == alvo))
        else {
            return "escolhe uma fonte acima".into();
        };
        match fonte.som {
            crate::audio::Som::Processo(_) => format!("vai o som de {}", fonte.rotulo),
            crate::audio::Som::TudoMenos(_) => {
                "vai o som da máquina, menos as vozes da call".into()
            }
            crate::audio::Som::Nenhum => {
                "vai sem som: não achei o Discord para deixar de fora".into()
            }
        }
    }

    fn bloco_do_balcao(&mut self, ui: &mut egui::Ui) {
        tema::bloco().show(ui, |ui| {
            // Transmitindo, a contagem vem da batida da propria transmissao: a
            // batida de fora fica parada enquanto isso, e o numero dela seria o
            // de minutos atras.
            let no_ar = matches!(self.estado, Estado::NoAr { .. });
            match &self.sala {
                Some(s) => {
                    ui.label(
                        egui::RichText::new(format!("sala {}", s.nome)).color(tema::TINTA),
                    );
                    let quantos = if no_ar { self.painel.pessoas() } else { s.quantos };
                    ui.label(tema::miudo(format!("{quantos} na call")));
                }
                None => {
                    ui.label(egui::RichText::new("fora da call").color(tema::TINTA));
                    ui.label(tema::miudo(
                        "entra numa call do servidor: a sala da tela é o canal de voz",
                    ));
                }
            }

            // A tela de outra pessoa não se toma: a trava é do site, e o botão
            // precisa dizer isso antes do clique, não depois do erro.
            let tomada = match &self.sala {
                Some(s) => s.dono_da_tela.clone().filter(|(_, sou_eu)| !sou_eu),
                None => None,
            };
            if let Some((quem, _)) = &tomada {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("a tela é de {quem} agora")).color(tema::AMBAR));
            }

            // Minimizada nao desenha nunca, e o que sairia e tela preta sem
            // erro nenhum. Melhor recusar com o motivo do que transmitir nada.
            let dormindo = self
                .escolhida
                .and_then(|alvo| self.fontes.iter().find(|f| f.alvo == alvo))
                .map(|f| f.minimizada)
                .unwrap_or(false);
            if dormindo {
                ui.add_space(6.0);
                faixa(
                    ui,
                    "essa janela esta minimizada: restaura ela antes, senao nao ha o que capturar",
                    tema::AMBAR,
                );
            }

            ui.add_space(10.0);
            self.botao_grande(ui, tomada.is_some() || dormindo);
        });
    }

    fn botao_grande(&mut self, ui: &mut egui::Ui, tela_tomada: bool) {
        let largura = ui.available_width();
        match self.estado {
            Estado::NoAr { .. } | Estado::Icando => {
                let botao = egui::Button::new(
                    egui::RichText::new("Baixar a tela").color(tema::TINTA),
                )
                .fill(tema::PAINEL_ALTO)
                .stroke(egui::Stroke::new(1.0, tema::BRASA));
                if ui.add_sized([largura, 42.0], botao).clicked() {
                    self.baixar();
                }
            }
            Estado::Baixando => {
                ui.add_enabled(
                    false,
                    egui::Button::new("soltando a tela…").min_size(egui::vec2(largura, 42.0)),
                );
            }
            _ => {
                let pode = self.escolhida.is_some() && self.sala.is_some() && !tela_tomada;
                let botao = egui::Button::new(
                    egui::RichText::new("Içar a tela").color(tema::BALCAO).strong(),
                )
                .fill(tema::AMBAR);
                if ui
                    .add_enabled_ui(pode, |ui| ui.add_sized([largura, 42.0], botao))
                    .inner
                    .clicked()
                {
                    self.icar();
                }
            }
        }
    }

    fn rodape(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);

        // O que a transmissao nao consegue fazer aparece AQUI. Antes isso ia
        // para o terminal, que num aplicativo de janela e o mesmo que nao ir a
        // lugar nenhum.
        if matches!(self.estado, Estado::Icando | Estado::NoAr { .. }) {
            let recado = self.painel.recado();
            if !recado.is_empty() {
                faixa(ui, &recado, tema::AMBAR);
                ui.add_space(6.0);
            }
        }

        if let Estado::NoAr { desde } = self.estado {
            let segundos = desde.elapsed().as_secs_f64();
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(painel::relogio(segundos as u64)).color(tema::AMBAR));
                ui.label(tema::miudo("·"));
                ui.label(tema::miudo(painel::frase_espectadores(
                    self.painel.espectadores(),
                )));
                ui.label(tema::miudo("·"));
                ui.label(tema::miudo(painel::tamanho(self.painel.bytes())));
                if !self.vazao.is_empty() {
                    ui.label(tema::miudo("·"));
                    ui.label(tema::miudo(format!("{} subindo", self.vazao)));
                }
            });
            // O aviso que mais importa: tela que não desenha não gera quadro, e
            // quem assiste fica na tela preta sem erro nenhum aparecer.
            if let Some(aviso) = painel::aviso_de_tela_parada(self.painel.pacotes(), segundos) {
                ui.add_space(6.0);
                faixa(ui, aviso, tema::BRASA);
            }
        }

        if let Some(aviso) = self.aviso.clone() {
            ui.add_space(6.0);
            faixa(ui, &aviso, tema::BRASA);
        }
        if let Some(relatorio) = self.relatorio.clone() {
            ui.add_space(6.0);
            ui.label(tema::miudo(format!("da última: {relatorio}")));
        }
    }
}

/// De quanto em quanto tempo perguntar à sala como estão as coisas.
///
/// Cada batida custa uma pergunta ao Discord (o estado de voz da pessoa), e o
/// site já pergunta de 5 em 5 segundos por conta dele. Com a janela de lado,
/// ninguém está lendo: ali a pressa não serve para nada e só gasta chamada.
/// Transmitindo, a própria transmissão bate o ponto, e esta batida só atrapalha.
pub fn intervalo_da_batida(focada: bool, transmitindo: bool) -> Duration {
    if transmitindo {
        Duration::from_secs(20)
    } else if focada {
        Duration::from_secs(8)
    } else {
        Duration::from_secs(45)
    }
}

/// Resume a presença no que a janela mostra.
fn resumir(p: crate::sala::Presenca) -> Resumo {
    let dono_da_tela = p.quem_transmite().map(|pessoa| {
        (
            pessoa.nome.clone().unwrap_or_else(|| pessoa.user_id.clone()),
            pessoa.user_id == p.eu,
        )
    });
    Resumo {
        nome: p.nome.clone().unwrap_or_else(|| "da call".into()),
        quantos: p.pessoas.len(),
        dono_da_tela,
    }
}

/// O selo da casa: um copo de chope, desenhado.
///
/// Desenhado e não imagem, como os ornamentos dos cards do bot: assim não
/// depende de arquivo nenhum e acompanha a cor do tema.
fn selo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 26.0), egui::Sense::hover());
    let p = ui.painter();
    let copo = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 2.0, rect.top() + 7.0),
        egui::pos2(rect.right() - 2.0, rect.bottom() - 1.0),
    );
    p.rect_filled(copo, egui::CornerRadius::same(3), tema::AMBAR_FOSCO);
    let cheio = egui::Rect::from_min_max(
        egui::pos2(copo.left(), copo.top() + 5.0),
        egui::pos2(copo.right(), copo.bottom()),
    );
    p.rect_filled(cheio, egui::CornerRadius::same(3), tema::AMBAR);
    let espuma = egui::Rect::from_min_max(
        egui::pos2(copo.left() - 1.0, rect.top()),
        egui::pos2(copo.right() + 1.0, rect.top() + 8.0),
    );
    p.rect_filled(espuma, egui::CornerRadius::same(4), tema::TINTA);
}

/// A pastilha de situação: bolinha e palavra.
fn pastilha(ui: &mut egui::Ui, texto: &str, cor: egui::Color32) {
    let fonte = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(texto.to_uppercase(), fonte, cor);
    let largura = galley.size().x + 26.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(largura, 22.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius::same(11), tema::PAINEL);
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(11),
        egui::Stroke::new(1.0, cor.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    p.circle_filled(egui::pos2(rect.left() + 11.0, rect.center().y), 3.5, cor);
    p.galley(
        egui::pos2(rect.left() + 19.0, rect.center().y - galley.size().y / 2.0),
        galley,
        cor,
    );
}

/// Uma faixa de aviso. Erro que não aparece na tela é erro que custa horas.
fn faixa(ui: &mut egui::Ui, texto: &str, cor: egui::Color32) {
    egui::Frame::new()
        .fill(cor.gamma_multiply(0.16))
        .stroke(egui::Stroke::new(1.0, cor.gamma_multiply(0.6)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(8)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(texto).small().color(tema::TINTA));
        });
}

/// Uma linha da lista: o nome em cima, o detalhe miúdo embaixo.
///
/// Desenhada à mão porque o rótulo de prateleira só aceita um texto, e duas
/// linhas são o que separa "Discord" de "Discord · 1920x1080": com duas telas
/// de mesma resolução ou dois navegadores abertos, o detalhe é a única coisa
/// que distingue uma fonte da outra.
fn linha_de_fonte(ui: &mut egui::Ui, fonte: &crate::fontes::Fonte, marcada: bool) -> bool {
    let largura = ui.available_width();
    let (rect, resposta) =
        ui.allocate_exact_size(egui::vec2(largura, 42.0), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let corpo = egui::TextStyle::Body.resolve(ui.style());
        let pequeno = egui::TextStyle::Small.resolve(ui.style());
        let p = ui.painter();
        if marcada {
            p.rect_filled(rect, egui::CornerRadius::same(6), tema::AMBAR.gamma_multiply(0.18));
            p.rect_stroke(
                rect,
                egui::CornerRadius::same(6),
                egui::Stroke::new(1.0, tema::AMBAR_FOSCO),
                egui::StrokeKind::Inside,
            );
        } else if resposta.hovered() {
            p.rect_filled(rect, egui::CornerRadius::same(6), tema::PAINEL_ALTO);
        }

        // O ícone diz o tipo sem gastar uma coluna: tela cheia é um retângulo,
        // janela é um retângulo com barra de título.
        let icone = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.center().y - 7.0),
            egui::vec2(18.0, 14.0),
        );
        let tinta_icone = if marcada { tema::AMBAR } else { tema::TINTA_FRACA };
        p.rect_stroke(
            icone,
            egui::CornerRadius::same(2),
            egui::Stroke::new(1.0, tinta_icone),
            egui::StrokeKind::Inside,
        );
        if !fonte.tela_inteira {
            p.rect_filled(
                egui::Rect::from_min_size(icone.min, egui::vec2(icone.width(), 4.0)),
                egui::CornerRadius::same(1),
                tinta_icone,
            );
        }

        let esquerda = rect.left() + 38.0;
        let sobra = (rect.right() - esquerda - 10.0).max(40.0);
        let nome = cortado(ui, &fonte.rotulo, corpo, tema::TINTA, sobra);
        let detalhe = cortado(ui, &fonte.detalhe, pequeno, tema::TINTA_FRACA, sobra);
        let p = ui.painter();
        p.galley(egui::pos2(esquerda, rect.top() + 4.0), nome, tema::TINTA);
        p.galley(
            egui::pos2(esquerda, rect.top() + 23.0),
            detalhe,
            tema::TINTA_FRACA,
        );
    }

    resposta.clicked()
}

/// Texto que não cabe sai com reticências, em UMA linha.
///
/// Sem isso um título longo (e título de navegador é sempre longo) vazaria por
/// cima do que está à direita, ou quebraria a linha e empurraria a lista.
fn cortado(
    ui: &egui::Ui,
    texto: &str,
    fonte: egui::FontId,
    cor: egui::Color32,
    largura: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut trabalho = egui::text::LayoutJob::simple_singleline(texto.to_string(), fonte, cor);
    trabalho.wrap.max_width = largura;
    trabalho.wrap.max_rows = 1;
    trabalho.wrap.break_anywhere = true;
    trabalho.wrap.overflow_character = Some('…');
    ui.painter().layout_job(trabalho)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn a_batida_desacelera_de_lado_e_para_quando_transmite() {
        // Cada batida pergunta ao Discord o estado de voz da pessoa. Com a
        // janela de lado ninguém está lendo, e transmitindo a própria
        // transmissão já bate o ponto.
        assert_eq!(intervalo_da_batida(true, false), Duration::from_secs(8));
        assert_eq!(intervalo_da_batida(false, false), Duration::from_secs(45));
        assert_eq!(intervalo_da_batida(true, true), Duration::from_secs(20));
        assert_eq!(intervalo_da_batida(false, true), Duration::from_secs(20));
    }
}
