# naru shell mockup

Naru 크롬 UI 목업 — **egui/eframe**로만 그린 UI 프로토타입 (CEF 없음).
레이아웃/디자인 언어 확인용이며, 중앙 패널은 추후 **CEF OSR 웹뷰(wgpu 합성)**로 교체된다.

## 디자인
- **형태**: init.Tasks 류 (모노스페이스·소문자·아이콘+라벨+카운트 행·섹션·점선 +new 버튼·airy)
- **내용**: 브라우저 (spaces / tabs[+agent tabs] / bookmarks / library[history·downloads·reading list])
- **팔레트**: Naru 고유 인디고/바이올렛 (쿨), **라이트/다크 토글**(☾/☀)
- 하단 **에이전트 바** 숨김 토글, 편집 가능 주소창, 한글 폴백(Malgun)

## 실행 (Windows)
```
# Rust(gnu) + MinGW(WinLibs) 필요. MinGW bin을 PATH에 둔 뒤:
cargo run
```
한 파일: `src/main.rs`.

> 상태: 레이아웃/색 방향 확정 단계. 버튼·아이콘셋·호버 등 디테일은 추가 폴리시 예정.
> 본체(Naru) 크로스플랫폼 설계/진척은 `../docs/naru-ownership-and-crossplatform-design.md` 참고.
