# naru-app — 실제 CEF 통합 브라우저 (Windows, 크로스플랫폼 증명)

Naru의 **진짜 동작하는 브라우저**. egui 셸 UI + **CEF OSR 웹 렌더링**을 한 창에 합성한다.
맥 없이 Windows에서 처음부터 끝까지 빌드/실행했다 (cef-rs = Chromium 기반, CEF 154).

셸 UI 프로토타입만 필요하면 `../shell-mockup/` 참고 (CEF 없는 egui 목업).

## 구조 / 기능
- **셸**: 프레임리스 상단바(nav·pill 주소창·테마 토글·창버튼), 인디고 팔레트, 라이트/다크
- **사이드바(init.Tasks식 3단계 트리)**: `spaces` → 영역(space) → 프로젝트(folder) → 탭
  - 각 단계 접기/펴기, `+ new space` / `+ new tab`로 유저가 생성
- **웹뷰**: 탭마다 독립 CEF 브라우저. 탭은 **활성화 시 lazy 생성**, 비활성은 `was_hidden`으로 렌더 중단(절전)
- **입력**: 마우스 이동/클릭/드래그/휠을 CEF로 전달
- **주소창**: Enter로 이동, nav(뒤로/앞으로/새로고침)
- **에이전트 컨트롤**: 에이전트 소유 탭이 활성화되면 하단 "agent in control" pill + 사이드바 run 배지
- **렌더링 최적화**: 적응형 repaint (변화 시 ~60fps, 유휴 시 ~15Hz) — 정적 페이지 사실상 idle

## 빌드 (Windows / MSVC)
cef-rs 워크스페이스의 `examples/naru`로 빌드한다. 필요:
- VS 2022 Build Tools (MSVC), Rust `stable-x86_64-pc-windows-msvc` 타깃
- VS Dev Shell에서(MinGW는 PATH에서 제외), VS 내장 cmake/ninja 사용
- `CEF_PATH` = export-cef-dir로 받은 CEF 154 windows64
- 빌드 후 CEF 런타임 파일(libcef.dll 등)을 실행파일 옆에 복사

```powershell
# VS Dev Shell + CEF_PATH 설정 후
cargo +stable-x86_64-pc-windows-msvc build --release -p naru
```

### 치명적 함정 (cef-rs)
1. `main()` 첫머리에 `api_hash(sys::CEF_API_VERSION_LAST, 0)` 필수 — 없으면 access violation
2. `Settings.root_cache_path`를 고유 경로로 지정 — 기본값이면 프로세스 싱글턴 충돌로 `initialize` 실패
3. CEF 서브프로세스(`--type=`)는 `execute_process` 후 즉시 return
4. OSR `on_paint`는 `shared_texture_enabled=false`에서 CPU 버퍼를 준다 (BGRA→RGBA 변환 후 egui 텍스처 업로드)

## 다음
키보드 입력(페이지 타이핑), 탭 제목/URL 자동 반영, 세션 영속, CEF 154→Naru 본체 156.
