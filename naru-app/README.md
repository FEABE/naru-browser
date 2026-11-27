# naru-app — 실제 CEF 통합 브라우저 (Windows, 크로스플랫폼 증명)

Naru의 **진짜 동작하는 브라우저**. egui 셸 UI + **CEF OSR 웹 렌더링**을 한 창에 합성한다.
맥 없이 Windows에서 처음부터 끝까지 빌드/실행했다 (cef-rs = Chromium 기반, CEF 154).

셸 UI 프로토타입만 필요하면 `../shell-mockup/` 참고 (CEF 없는 egui 목업).

## 구조 / 기능
- **셸**: 프레임리스 상단바(nav·pill 주소창·테마 토글·창버튼), 인디고 팔레트, 라이트/다크
- **사이드바(init.Tasks식 3단계 트리)**: `spaces` → 영역(space) → 프로젝트(folder) → 탭
  - 각 단계 접기/펴기, `+ new space` / `+ new tab`로 유저가 생성
- **웹뷰**: 탭마다 독립 CEF 브라우저. 탭은 **활성화 시 lazy 생성**, 비활성은 `was_hidden`으로 렌더 중단(절전)
- **탭 닫기**: 사이드바 행 hover 시 `✕` → `close_browser`로 CEF 서브프로세스까지 회수
- **입력**: 마우스(이동/클릭/드래그/휠) + 키보드를 CEF로 전달. 키보드는 **영어·한글 모두** 지원 —
  문자는 `Event::Text`/IME 커밋을 CHAR로, 특수키(Enter/Backspace/화살표 등)는 VK로. 프레임리스 창에 `IMEAllowed` 세팅해 한글 조합 입력 가능(유저 탭·옴니박스). 라이브 조합 프리뷰는 CHAR 에코 방식(아래 참조)
- **커서 동기화**: OSR은 OS가 웹 커서를 모르므로 `on_cursor_change`(DisplayHandler)로 받아 egui 커서로 매핑(링크=손가락, 입력=I-beam, 리사이즈 방향 등)
- **팝업/새 창 → 새 탭**: `on_before_popup`(LifeSpanHandler)으로 CEF 팝업을 취소하고 대상 URL을 새 탭으로 연다. 사용자 제스처(클릭)로 연 것만 승격(스크립트 자동 팝업 차단). 새 탭은 연 탭의 owner/area/proj 상속
- **HiDPI**: `ctx.pixels_per_point()`를 매 프레임 추적해 `screen_info`의 device_scale_factor로 보고 → 고배율 디스플레이에서 선명(배율 변경 시 notify_screen_info_changed). view_rect·마우스 좌표는 논리 단위 유지, paint 버퍼만 배율²로 커짐
- **단축키/확대축소**: Ctrl+T 새 탭, Ctrl+W 닫기, Ctrl+Tab/Ctrl+Shift+Tab 전환, Ctrl+L 주소창 포커스, Ctrl +/-/0 확대·축소(CEF `set_zoom_level`, 탭별 유지). egui `consume_key`로 CEF 전달 전에 가로챔
- **주소창**: Enter로 이동, nav(뒤로/앞으로/새로고침)
- **에이전트 컨트롤**: 에이전트 소유 탭이 활성화되면 하단 "agent in control" pill + 사이드바 run 배지.
  CDP(9222)는 **opt-in**: 환경변수 `NARU_AGENT=1`로 실행할 때만 포트 노출(기본 비노출, `NARU_CDP_PORT`로 포트 지정 가능)
- **메모리/렌더 최적화**: 숨긴 탭은 CPU 픽셀 버퍼 해제, 프레임은 BGRA→`Color32` 1패스 변환(복사 2회→1회+memcpy).
  적응형 repaint(변화 시 ~60fps, 유휴 시 ~15Hz) — 정적 페이지 사실상 idle
- **경로**: 메일박스·응답 등 임시 파일은 OS temp 기반(하드코딩 없음)
- **세션 영속**: `%LOCALAPPDATA%\naru\session.txt`에 탭/스페이스 저장→재시작 시 복원. `root_cache_path`를 영구화해 쿠키/로그인 유지(정상 종료 시 flush)
- **한글 입력 라이브 프리뷰**: CEF `ime_set_composition`이 이 OSR 환경에선 화면에 안 떠서, 조합 글자를 CHAR로 미리 그리고(`ime_comp` 추적) 조합이 바뀌면 Backspace 후 재입력하는 방식으로 실시간 프리뷰 구현(ㅎ→하→한). 확정도 동일 CHAR 경로라 확실

## 빌드 (Windows / MSVC)
cef-rs 워크스페이스의 `examples/naru`로 빌드한다. 필요:
- VS 2022 Build Tools (MSVC), Rust `stable-x86_64-pc-windows-msvc` 타깃
- VS Dev Shell에서(MinGW는 PATH에서 제외), VS 내장 cmake/ninja 사용
- `CEF_PATH` = export-cef-dir로 받은 CEF 154 windows64
- 빌드 후 CEF 런타임 파일(libcef.dll 등)을 실행파일 옆에 복사

```powershell
# VS Dev Shell + CEF_PATH 설정 후
cargo +stable-x86_64-pc-windows-msvc build --release -p naru

# 일반 실행 (CDP 비노출)
.\target\release\naru.exe
# 에이전트 모드 (CDP 9222 노출 → agent-demo.mjs 등으로 제어)
$env:NARU_AGENT="1"; .\target\release\naru.exe
```

### 치명적 함정 (cef-rs)
1. `main()` 첫머리에 `api_hash(sys::CEF_API_VERSION_LAST, 0)` 필수 — 없으면 access violation
2. `Settings.root_cache_path`를 고유 경로로 지정 — 기본값이면 프로세스 싱글턴 충돌로 `initialize` 실패
3. CEF 서브프로세스(`--type=`)는 `execute_process` 후 즉시 return
4. OSR `on_paint`는 `shared_texture_enabled=false`에서 CPU BGRA 버퍼를 준다 (→ `Color32` 변환 후 egui 텍스처 업로드)

## 다음
키보드 입력(페이지 타이핑), 탭 제목/URL 자동 반영, 세션 영속, CEF 154→Naru 본체 156.
