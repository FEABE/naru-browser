# Naru — 소유권 모델 & 크로스플랫폼 설계 (ego lite 분석 기반)

> 작성 2026-11-24. ego lite(`package/ego-browser`, TS/CDP 하네스)의 공개 API를 분석해
> Naru(Rust+CEF)의 소유권/제어 API와 크로스플랫폼 경로를 정리한 설계 메모.

## 0. 목표
- Naru를 **사람과 AI가 한 브라우저를 나눠 쓰는 독립 브라우저**로 완성(ego lite 수준).
- **"어디서든"**: 맥 없이 Linux/Windows에서도 빌드·실행. (현재 코드는 macOS 전용)
- Rust 선택 이유 = **경량·고속 제어 플레인**(아래 실측 근거).

## 1. 오늘 확인된 사실(근거)
- **포터블 코어는 이미 크로스플랫폼**: `cargo test --no-default-features` → Linux에서 **193개 통과**
  (`protocol`/CDP 와이어, `history`, `bookmarks`, `site_permissions`). macOS에 묶인 건 UI 셸뿐.
- **Rust 제어 플레인 경량성(서버 실측)**:
  | | Rust `lightbrowser` | Node v22(빈 스크립트 바닥) |
  |---|---|---|
  | 크기 | 740 KB 단일 바이너리 | 203 MB 런타임 |
  | 콜드스타트 | ~0 ms | 41.6 ms |
  | peak RSS | ~1.9 MB | 41.2 MB |
  - 단, 브라우저 엔진(Chromium/CEF) RAM(수백 MB)은 Rust/Node 무관 동일. Rust 이득은 **제어/드라이버·기동·상주** 영역.
- **cef-rs(vendored)**: `accelerated_osr`(wgpu+ash, Linux dmabuf) 지원하나, **바인딩/리소스가 macOS만** 생성돼 있음
  (`bindings/mod.rs`가 `cfg(target_os="macos")`만 include). **CEF Views는 미바인딩**(`window_info.rs`만 존재).

## 2. 타깃 아키텍처 (3층)
```
┌─ 포터블 코어 (Rust, OS 무관) ───────────────────────────────┐
│  protocol(CDP) · history · bookmarks · site_permissions     │  ← 이미 193 테스트 녹색
│  TaskSpace/Page/Ownership 상태기계 (아래 §3)                 │
└──────────────┬──────────────────────────────────────────────┘
               │ trait 경계 (NativeShell / CdpTransport)
┌──────────────┴─ 네이티브 셸 (플랫폼별) ─────────────────────┐
│  macOS: AppKit (현행 src/main.rs)                            │
│  Linux/Win: OSR + winit + wgpu(자체 chrome, egui 등)  ← 신규 │
└──────────────┬──────────────────────────────────────────────┘
               │ CEF (Chromium 엔진, 크로스플랫폼)
└─ vendor/cef-rs (+ Linux/Win 바인딩 추가 필요) ───────────────┘
```
원칙(ego lite에서 차용): **지능은 전부 포터블 코어에, 네이티브 셸은 "창 + CDP 전송 + 소유권 상태"만** 노출.

## 3. 소유권 모델 (ego lite → Naru 차용)
### 상태 (3-state)
```
Ownership = User | Agent | AgentDelegatedToUser
agent_owned(o) == (o == Agent || o == AgentDelegatedToUser)
```
### 전이 규칙 (ego lite helpers.ts 정책 그대로)
| 연산 | user 소유 공간 | agent 소유 공간 |
|---|---|---|
| `task_space(name)` 이름으로 | `NameInUse` 에러 | 재사용 |
| `task_space(id)` id로 | **미claim 선택**(UserInControl 에러 표면) | 선택 |
| `switch_task_space` | 에러(agent 전용) | 선택 |
| `claim_task_space(id)` | **user→agent 인수 후 선택** | - |
| `take_over_task_space(id)` | 소유권 체크 없이 강제 | 강제 |
| `hand_off()` | - | 사용자에게 반환 |
| `finish({keep})` | claim 후 닫기 | 종료 |
- **UX 불변식**: user 소유 공간은 **AI가 임의로 뺏지 않는다**. 사용자 동의 후에만 `claim`.
- 에러 코드: `TaskSpaceNameInUse`, `TaskSpaceUserInControl`, 종료된 태스크는 `take_over`로 재개 불가(하드스톱).

## 4. Rust API 표면 제안 (ego lite TS 매핑)
```rust
// 공간 수명주기
async fn profiles() -> Vec<Profile>;
async fn list_task_spaces() -> Vec<TaskSpaceInfo>;
async fn task_space(loc: Locator, opts: NewOpts) -> TaskSpace; // name|id
async fn claim_task_space(loc: Locator) -> TaskSpace;
async fn take_over_task_space(id: SpaceId) -> TaskSpace;

impl TaskSpace {
    fn space_id(&self) -> SpaceId; fn name(&self) -> &str; fn ownership(&self) -> Ownership;
    fn page(&self, label: &str) -> Page;
    async fn user_page(&self) -> Option<Page>;     // claim/takeover 경계의 사용자 탭
    async fn pages(&self) -> Vec<Page>; async fn tabs(&self) -> Vec<TabInfo>;
    async fn new_page(&self) -> Page;
    async fn adopt(&self, p: UnmanagedPage, as_: Option<&str>) -> Page;
    async fn release(&self, label: &str);
    async fn wait_for_control(&self, interval: Dur, timeout: Dur);
    async fn hand_off(&self); async fn finish(&self, keep: bool);
    async fn cdp(&self, method: &str, params: Value, timeout: Dur) -> Value; // 밑바닥
}

impl Page { // 전부 cdp() 위에 구현
    // 식별: label, space_id, opened_by, target_id
    async fn goto(&self, url, opts); async fn reload(&self, opts); async fn url(&self);
    async fn wait_for_url / wait_for_selector / wait_for_load_state / wait_for_event / wait_for_timeout;
    async fn title / info / evaluate / fetch / cdp;
    async fn fill / select_option / focus / press / set_input_files;
    async fn accept_dialog / dismiss_dialog;
    mouse: move/down/up/wheel;  keyboard: down/up/press/type/insert_text/paste;
    async fn wait_for_file_chooser() -> FileChooser; async fn close();
}
// Download: page/url/suggested_filename/save_as/path/failure/cancel/delete
```
핵심: **모든 조작은 `cdp()` 한 점으로 수렴** → OS/셸 무관(ego lite의 `globalThis.ego.sendCDPMessage`와 동형).

## 5. 크로스플랫폼 UI 경로 = OSR + wgpu/winit
CEF Views가 미바인딩이므로 **OSR(off-screen rendering)**로 간다:
1. CEF를 OSR 모드로 → 렌더 결과를 GPU 텍스처(Linux dmabuf, cef-rs `accelerated_osr` 지원)로 수신.
2. `winit` 창 + `wgpu`로 텍스처 합성, chrome UI는 `egui` 등으로 자체 그림.
3. 입력은 winit → CEF로 주입. 이 경로는 Win/Linux/mac 공통.

## 6. Linux 빌드 블로커 & 해소 순서 (오늘 실증)
현 상태: vendored cef-rs가 **macOS 바인딩만** 포함 + clang 미설치 + sudo 비번필요.
1. `sudo apt install -y clang libclang-dev`  (bindgen 의존)
2. upstream `tauri-apps/cef-rs`(pinned `2861f69…`)의 **`update-bindings` 툴** 확보(이 벤더 카피엔 빠져있음).
3. **Linux CEF 156 SDK**(동일 버전 `156.0.1+…chromium-156.0.8078.17`) 헤더 확보(cef-builds.spotifycdn.com; 런타임은 `download_cef`가 자동 취득).
4. `BINDGEN_EXTRA_CLANG_ARGS=-DCEF_API_VERSION=15600 update-bindings --bindgen --target x86_64-unknown-linux-gnu`
   → `x86_64_unknown_linux_gnu.rs` 바인딩/리소스 생성.
5. `bindings/mod.rs`·`resources/mod.rs`에 `cfg(target_os="linux")` 모듈 추가.
6. **Linux 엔트리포인트 신규**: `src/main.rs`(AppKit)는 Linux 빌드 불가 → OSR+winit 셸을 별도 bin으로.
7. `--features accelerated_osr` 로 빌드.

## 7. 마일스톤
- M1. (완료) 포터블 코어 Linux 컴파일+테스트 ✅
- M2. (완료) Linux CEF 바인딩 이식(upstream x86_64_unknown_linux_gnu) → cef+cef-dll-sys Linux 빌드 통과 ✅ (2026-11-24)
- M3. OSR+winit 최소 셸: 창에 about:blank 렌더 + 입력 주입
- M4. 포터블 코어의 TaskSpace/Ownership를 셸에 연결(§3~4)
- M5. ego lite parity E2E(14 여정) 중 Linux 가능 항목부터
- (mac 생기면) AppKit 셸은 기존 그대로 병행 타깃

## 8. 크롬 UI 설계 (Aside 참고 + ego lite 소유권) — M3 대상

> Aside(aside.com) 실제 UI 관찰 + ego lite TaskSpace/ownership + Naru README 기능을 결합.
> 에셋 복사 아님 — 패턴/레이아웃만 참고해 우리 스타일로 재구현.

### 관찰된 Aside 크롬
- 좌측 사이드바(접이식): 프로필 전환 → 핀 앱 그리드(3×2) → Bookmarks → Chats/Tasks(상태 스피너·점) → Tabs(New Tab / Agent tabs(n) / 사용자 탭)
- 상단바: 뒤로/앞으로/새로고침/찾기 + 주소창 + 창 컨트롤
- 새탭/커맨드 화면: Search|Ask 입력 + 컨트롤 행( + / Local▾[작업폴더] / Guard▾[권한모드] / 모델 / 강도▾ )
- Recent tasks 카드: 작업 상태+하위단계, 예약 작업

### Naru 크롬 (egui를 OSR 웹 텍스처 위에 얹음)
레이아웃:
- **좌측 레일(접이식)** = egui `SidePanel::left`
  - **Space 전환기** = ego lite TaskSpace. 각 Space에 **소유권 배지(User/Agent/Delegated)** 표시
  - 핀 사이트 그리드
  - Bookmarks
  - **Spaces & Tasks** 리스트: 상태(idle / working / needs-confirmation) — ego lite TaskSpace + Aside 태스크 융합
  - **세로 탭**: 사용자 탭 + **"Agent tabs(n)" 그룹(에이전트 소유 페이지 시각 분리)** → "AI가 사용자 탭을 뺏지 않음" 불변식을 UI로 강제
- **상단바** = egui `TopBottomPanel::top`: 뒤로/앞으로/새로고침/찾기 + 주소창(= Search|Ask 커맨드 겸용) + 창 컨트롤
- **중앙** = egui `CentralPanel`: **CEF OSR 텍스처를 wgpu로 렌더**(웹 본문). 크롬 위젯이 그 주변을 둘러쌈
- **새탭/커맨드 서피스**: Search|Ask 토글 + 컨트롤 행(작업폴더 / 권한모드 / 모델 / 강도)
- **태스크 카드**: 실시간 에이전트 작업 상태+하위단계+**중지 버튼**(Naru README의 "task status and stop button")

소유권 시각 규칙(차별점, ego lite에서):
- 에이전트 소유 탭/페이지는 그룹화+배지. 사용자 Space는 임의 탈취 금지.
- 민감 작업(결제/게시/메시지)은 크롬에서 **확인 프롬프트**(Aside "Guard"/confirmation + ego lite ownership).

기술:
- egui(즉시모드) = 크로스플랫폼·빠른 반복. 패널 3분할(좌 사이드 / 상단 / 중앙).
- 중앙 CentralPanel = wgpu 서피스에 OSR 웹 텍스처 합성(§5 파이프라인), 입력은 winit→CEF 주입.
- 전부 OS 무관(Win/Linux/mac 동일 크롬 코드) → "어디서든" 달성.

### UI 마일스톤 (M3 세분화)
- M3a. OSR+winit 창에 웹 텍스처 렌더(크롬 없음)
- M3b. egui 3패널 셸 + 주소창/탭(정적)
- M3c. 세로 탭·Space 전환·소유권 배지(코어 연결)
- M3d. 커맨드 서피스(Search|Ask) + 권한모드/모델/작업폴더
- M3e. 태스크 카드(에이전트 상태·중지)

### 탭 레이아웃 모드 (크롬식 가로 + Arc/Aside식 세로, 전환식)

요구사항: 사용자가 **세로 탭 ↔ 가로 탭**을 전환할 수 있어야 함.

- `enum TabLayout { Horizontal, Vertical }` — 설정 + 단축키로 토글. 상태는 창별로 저장.
- **하나의 탭 모델**(동일 `Tab` 구조·소유권 배지·Agent/User 그룹)에 **렌더 위치만 다른 두 뷰**. 데이터는 공유, 뷰만 분기.

**Horizontal (크롬 스타일)**
- egui `TopBottomPanel::top` 안에 **가로 탭 스트립**(탭 칩 행, 넘치면 스크롤 + `+` 새탭). 주소창은 그 아래 줄.
- 좌측 사이드바는 접거나 축소(Space/북마크만) 가능. 탭은 상단에.

**Vertical (Arc/Aside 스타일)**
- egui `SidePanel::left` 안에 **세로 탭 리스트**(사용자 탭 / "Agent tabs(n)" 그룹). 현행 §8 레이아웃.
- 상단바는 주소창+창컨트롤만(탭 없음).

**공통**
- 탭 선택/닫기/순서변경/드래그, Agent·User 소유권 배지, 번호 선택·탭 순환은 **두 모드 동일 동작**(표시 순서만 따름).
- 전환 시 현재 탭 선택·주소 초안·AI 제어권 보존.
- 기본값: Vertical(에이전트·Space 모델에 적합) 권장, 단 Horizontal로 크롬 친숙도 제공.

구현 포인트: 탭 위젯을 `render_tab_strip_horizontal(ui, &tabs)` / `render_tab_list_vertical(ui, &tabs)` 두 함수로 분리하고, `TabLayout`에 따라 상단 패널 또는 좌측 패널에서 호출. 중앙 OSR 웹 뷰·코어 로직은 불변.

### 비주얼 언어 = 사용자 본인 디자인 킷 (init.Tasks / vanilla 공유 킷)

실제 레퍼런스는 Aside가 아니라 **사용자의 init.Tasks**(app.inittasks.com) — 본인 공유 컴포넌트 킷.
→ Naru 크롬의 **기능 구조는 Aside/ego lite**, **비주얼/인터랙션 언어는 사용자 킷**으로.

관찰된 디자인 언어(init.Tasks):
- **모노스페이스 폰트 + 소문자 라벨**, 플랫/미니멀, 밝은 테마, **연두(green) 액센트**(today 하이라이트·now 라인).
- **좌측 사이드바**: 상단 프로필 전환(`personal ▾`)+접기 아이콘 → **아이콘+라벨 리스트**(섹션: inbox / today·planned·anytime·someday / calendar·all tasks·attachments / projects(+) / notes(+) / tags(+) / filters(+) / completed / trash), 우측 **카운트 배지**, 하단 account·settings.
- **상단 툴바**: 좌측 제목 + 우측 **아이콘 버튼 행**(filter/sort/info/edit/search/calendar/panel-toggle).
- **하단 빠른입력**: `> add to inbox…` + `+`.
- **3-pane**: 사이드 / 리스트(중앙) / 보조 패널(우측, 예: 캘린더 day 뷰).

Naru 적용:
- 탭·주소창·Space·태스크 카드 전부 **이 모노스페이스·소문자·연두액센트 스타일**로 그림(egui 테마를 이 킷에 맞춤: 폰트=모노, 텍스트=소문자 규칙, accent=green, 플랫 보더).
- 사이드바는 init.Tasks식 **아이콘+라벨+카운트 배지** 리스트. 세로탭도 이 리스트 스타일, Agent tabs 그룹도 동일 톤.
- 상단바 아이콘 버튼 = init.Tasks 툴바 톤. 하단 커맨드 입력(`> …`)도 동일.
- 우측 보조 패널 = Aside의 side panel(현재 페이지를 태스크 컨텍스트로) 자리 → init.Tasks의 3번째 pane 레이아웃 그대로 활용 가능.
