# Tela do Boteco

App de transmissão de tela do **Boteco Copo Sujo**. Captura uma janela ou um
monitor e manda para quem está na mesma call, pelo site da Tela.

## Por que existe

Não é por qualidade de imagem: é por **som**. O navegador não consegue capturar
o áudio de um aplicativo só. O `getDisplayMedia` no Windows oferece "áudio da
aba" ou "áudio do sistema inteiro", e nada entre os dois. Sistema inteiro inclui
o Discord, então quem assiste ouve a própria voz de volta com atraso. É limite da
API, e não tem contorno dentro do navegador.

No Windows existe loopback **por processo**, que captura o som de um aplicativo
ou de tudo menos ele. Isso foi testado antes de uma linha ser escrita: com
música tocando no Spotify e uma call do Discord soando ao lado, a captura por
processo gravou **só a música**.

Quem não quiser instalar nada continua compartilhando pelo site, com a limitação
do som. O app é a opção para quem quer o som certo.

## Decisões

| | |
|---|---|
| **Rust com interface nativa** | nada de motor de navegador: quem transmite está jogando, e o app não pode disputar memória com o jogo |
| **O núcleo não sabe nada da interface** | captura, codificação e rede moram numa biblioteca que a casca só comanda, então trocar a interface não mexe no motor |
| **Escolher janela ou monitor**, como no Discord | e a janela escolhida entrega o processo, que é de onde o áudio vem |
| **A janela fecha para a bandeja** | durante o jogo não há interface desenhando nada |
| **Codificação em hardware** | NVENC, QuickSync ou AMF, sem o quadro passar pela CPU |

## Estado

Em construção. O que já funciona:

- [x] Listar as janelas transmissíveis, com o processo dono de cada uma
- [ ] Capturar uma janela (Windows Graphics Capture)
- [ ] Codificar em hardware
- [ ] Áudio do processo escolhido
- [ ] Mandar para o site (WebRTC)
- [ ] Interface e bandeja

## Compilar

Precisa de Rust. No Windows, qualquer um dos dois toolchains serve:

```bash
cargo build --release
```

O executável sai em `target/release/tela-boteco.exe`.
