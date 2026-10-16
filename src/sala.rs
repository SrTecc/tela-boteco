//! A conversa com o site: presença e caixa postal.
//!
//! O app não inventa protocolo nenhum. Ele fala exatamente o que a página de
//! quem assiste já fala, porque a regra é que **quem assiste não pode saber se
//! o vídeo veio do navegador ou do app**. São dois endereços:
//!
//! - `/api/sala` é a batida de presença. Ela prova a cada ciclo que a pessoa
//!   continua na call, perguntando ao Discord, e é dela que sai a lista de quem
//!   está na sala. Também é onde se pede a tela (`transmitindo=1`).
//! - `/api/sinal` é a caixa postal: oferta, resposta e candidatos de rede. O
//!   vídeo nunca passa por lá.
//!
//! A sessão vai como cookie, montado à mão. O site a emitiu no login; aqui ela
//! só viaja de volta.

use serde::Deserialize;
use std::time::Duration;

pub struct Sala {
    site: String,
    sessao: String,
    cliente: reqwest::Client,
}

/// O que a batida de presença devolve.
#[derive(Debug, Deserialize)]
pub struct Presenca {
    pub sala: String,
    #[serde(default)]
    pub nome: Option<String>,
    pub eu: String,
    #[serde(default)]
    pub pessoas: Vec<Pessoa>,
    /// `false` quando outra pessoa já está transmitindo. A trava de uma tela
    /// por vez mora no banco do site, não aqui: dois cliques no mesmo instante
    /// passariam pelos dois lados sem nenhum saber do outro.
    ///
    /// Opcional porque o site devolve **nulo** para quem nunca pediu a tela,
    /// e não `false`. Exigir booleano aqui derrubava a leitura inteira por
    /// causa de um campo que nem é o principal.
    #[serde(default)]
    pub no_ar: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Pessoa {
    pub user_id: String,
    #[serde(default)]
    pub nome: Option<String>,
    /// Nulo no banco para quem nunca transmitiu. Ver a nota em `no_ar`.
    #[serde(default)]
    pub transmitindo: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Sinal {
    /// O site chama este campo de `de_id`. Chamar de `de` aqui sem dizer a
    /// ele custou uma rodada de teste: a leitura inteira falhava com "error
    /// decoding response body", que nao aponta para campo nenhum.
    #[serde(rename = "de_id")]
    pub de: String,
    pub tipo: String,
    #[serde(default)]
    pub dados: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RespostaSinais {
    #[serde(default)]
    sinais: Vec<Sinal>,
}

/// O que deu errado, separando "não é problema seu" de "não dá para continuar".
#[derive(Debug)]
pub enum Erro {
    /// Fora da call: não é falha, é estado. A pessoa precisa entrar numa call.
    ForaDaCall,
    /// Sessão vencida ou inválida: precisa entrar de novo.
    SemSessao,
    Rede(String),
}

impl std::fmt::Display for Erro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Erro::ForaDaCall => write!(f, "tu não estás numa call do servidor"),
            Erro::SemSessao => write!(f, "a sessão venceu, entra de novo"),
            Erro::Rede(m) => write!(f, "rede: {m}"),
        }
    }
}

impl std::error::Error for Erro {}

impl Presenca {
    /// Posso transmitir agora? Nulo conta como sim: significa que ninguém
    /// reivindicou a tela.
    pub fn tela_livre(&self) -> bool {
        self.no_ar.unwrap_or(true)
    }
}

impl Pessoa {
    pub fn esta_transmitindo(&self) -> bool {
        self.transmitindo.unwrap_or(false)
    }
}

impl Sala {
    pub fn nova(site: &str, sessao: &str) -> Result<Self, Erro> {
        let cliente = reqwest::Client::builder()
            // Prazo curto: a batida é a cada 5s, e uma requisição pendurada por
            // mais que isso já perdeu a vez.
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| Erro::Rede(e.to_string()))?;
        Ok(Self {
            site: site.trim_end_matches('/').to_string(),
            sessao: sessao.to_string(),
            cliente,
        })
    }

    /// Bate o ponto. Com `transmitindo`, também pede a tela.
    pub async fn bater_ponto(&self, transmitindo: bool) -> Result<Presenca, Erro> {
        let url = format!(
            "{}/api/sala{}",
            self.site,
            if transmitindo { "?transmitindo=1" } else { "" }
        );
        let resposta = self
            .cliente
            .get(&url)
            .header("Cookie", format!("tela_sessao={}", self.sessao))
            .send()
            .await
            .map_err(|e| Erro::Rede(e.to_string()))?;

        match resposta.status().as_u16() {
            200 => {
                // O site chama o campo de `noAr`; aqui ele é `no_ar`. Converter
                // na borda evita espalhar o nome de lá pelo código todo.
                let bruto: serde_json::Value = resposta
                    .json()
                    .await
                    .map_err(|e| Erro::Rede(e.to_string()))?;
                let mut valor = bruto.clone();
                if let Some(no_ar) = bruto.get("noAr") {
                    valor["no_ar"] = no_ar.clone();
                }
                serde_json::from_value(valor).map_err(|e| Erro::Rede(e.to_string()))
            }
            401 => Err(Erro::SemSessao),
            403 => Err(Erro::ForaDaCall),
            outro => Err(Erro::Rede(format!("o site respondeu {outro}"))),
        }
    }

    /// Lê a caixa postal. Devolve vazio quando não há recado.
    pub async fn ler_sinais(&self) -> Result<Vec<Sinal>, Erro> {
        let resposta = self
            .cliente
            .get(format!("{}/api/sinal", self.site))
            .header("Cookie", format!("tela_sessao={}", self.sessao))
            .send()
            .await
            .map_err(|e| Erro::Rede(e.to_string()))?;

        match resposta.status().as_u16() {
            200 => {
                // Le como TEXTO e so entao converte: quando o formato muda
                // do outro lado, a mensagem precisa mostrar o que chegou.
                // "error decoding response body" sozinho nao aponta para
                // campo nenhum, e custou uma rodada de teste.
                let texto = resposta.text().await.map_err(|e| Erro::Rede(e.to_string()))?;
                let corpo: RespostaSinais = serde_json::from_str(&texto).map_err(|e| {
                    Erro::Rede(format!("{e} | o site mandou: {}", &texto.chars().take(200).collect::<String>()))
                })?;
                Ok(corpo.sinais)
            }
            401 => Err(Erro::SemSessao),
            403 => Err(Erro::ForaDaCall),
            outro => Err(Erro::Rede(format!("o site respondeu {outro}"))),
        }
    }

    /// Põe um recado na caixa de alguém.
    pub async fn mandar(
        &self,
        para: &str,
        tipo: &str,
        dados: serde_json::Value,
    ) -> Result<(), Erro> {
        let resposta = self
            .cliente
            .post(format!("{}/api/sinal", self.site))
            .header("Cookie", format!("tela_sessao={}", self.sessao))
            .json(&serde_json::json!({ "para": para, "tipo": tipo, "dados": dados }))
            .send()
            .await
            .map_err(|e| Erro::Rede(e.to_string()))?;

        match resposta.status().as_u16() {
            200 => Ok(()),
            401 => Err(Erro::SemSessao),
            403 => Err(Erro::ForaDaCall),
            outro => Err(Erro::Rede(format!("o site respondeu {outro}"))),
        }
    }

    /// Avisa que saiu, para a sala não ficar com fantasma até a poda.
    pub async fn sair(&self) {
        let _ = self
            .cliente
            .get(format!("{}/api/sala?sair=1", self.site))
            .header("Cookie", format!("tela_sessao={}", self.sessao))
            .send()
            .await;
    }
}
