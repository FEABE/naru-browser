# Naru

Naru는 macOS용 Rust, CEF 브라우저 시제품입니다. CEF에 포함된 Chromium으로 웹페이지를 표시하고, Rust CLI가 실행 중인 앱에 로컬 소켓으로 명령을 전달합니다. Electron, Node 런타임, MCP 서버는 사용하지 않습니다.

현재 범위는 브라우저 실행과 CLI 제어입니다. 내장 AI 채팅과 모델 API 연결은 아직 구현하지 않았습니다.

저장소와 프로젝트 이름은 Naru입니다. 현재 검증된 시제품의 앱 번들은 `Light Browser.app`, CLI는 `lightbrowser`이며, 프로필과 키체인 식별자도 기존 이름을 사용합니다.

## 실행

필요한 도구는 Rust, Cargo, Xcode Command Line Tools, CMake, Python 3입니다. macOS ARM64에서 실행을 검증했습니다. Intel용 배포 구성은 준비돼 있지만 실제 실행은 확인하지 않았습니다.

```bash
git clone https://github.com/FEABE/naru-browser.git
cd naru-browser
./scripts/dev.sh --release
```

최초 실행 시 고정된 CEF 156.0.1 beta 런타임을 `.cache/cef`에 내려받습니다. Ninja가 설치돼 있지 않으면 검증한 공식 바이너리를 `.cache/bin`에 받습니다. CEF는 공식 배포 인덱스의 파일 크기와 SHA-1, Ninja는 고정한 공식 SHA-256을 확인합니다. 전역 설치는 하지 않습니다. Cargo는 기본 Cargo 캐시를 사용합니다.

생성된 앱은 `dist/Light Browser.app`입니다. `--build-only`를 추가하면 실행하지 않고 앱만 만듭니다. 개발용 ad hoc 서명이며 배포용 공증은 하지 않았습니다.

**수정한 실행 문제:** CEF 156의 앱 전용 키체인 API를 적용했습니다. `Light Browser Safe Storage` / `local.lightbrowser.dev` 이름을 사용하며 다른 Chromium 앱의 키를 읽지 않습니다. 이전 시제품의 `profile`은 그대로 두고 새 `profile-v1`을 사용하므로 기존 로그인 데이터를 자동 이전하지 않습니다.

Chromium 로그는 `~/Library/Logs/Light Browser/chromium.log`에 저장합니다. Documents 안의 로그 파일을 여는 단계에서 GPU 보조 프로세스가 대기하는 현상이 확인돼, 프로필 위치와 로그 위치를 분리했습니다.

개발용 ad hoc 서명은 코드가 바뀌면 macOS에서 같은 앱으로 인정되지 않을 수 있습니다. 재빌드 후 앱 전용 키체인 접근 승인을 다시 요청할 수 있으며 이 승인은 사용자가 직접 처리해야 합니다. 배포용 서명과 공증은 아직 준비하지 않았습니다.

## CLI

`scripts/dev.sh`는 브라우저 데이터를 프로젝트의 `.local-data`에 저장합니다. 다른 터미널의 CLI에도 같은 경로를 설정합니다.


```bash
export LIGHT_BROWSER_DATA_DIR="$PWD/.local-data"
./target/release/lightbrowser status
./target/release/lightbrowser open https://www.naver.com
./target/release/lightbrowser tabs
```

`open` 응답의 `result.tab`을 이후 명령에 사용합니다. 아래 `3`은 예시입니다.

```bash
./target/release/lightbrowser read 3
./target/release/lightbrowser fill 3 'read에서 받은 ref' '가벼운 브라우저'
./target/release/lightbrowser press 3 '같은 ref' Enter
./target/release/lightbrowser read 3
./target/release/lightbrowser screenshot 3 "$PWD/artifacts/example.png"
./target/release/lightbrowser close 3
```

스크린샷의 상위 폴더는 미리 만들어야 하며 기존 파일을 덮어쓰지 않습니다. 모든 명령은 stdout에 JSON 하나를 반환합니다. 오류는 stderr에도 표시하며 0이 아닌 종료 코드를 사용합니다. `help`에서 명령 목록을 확인할 수 있습니다. CLI는 `.app/Contents/MacOS/lightbrowser`에도 포함됩니다.

앱을 Finder에서 직접 열면 기본 프로필은 `~/Library/Application Support/Light Browser`입니다. 이때 CLI에서도 `LIGHT_BROWSER_DATA_DIR`을 해제하거나 같은 프로필 경로를 지정합니다. 다른 브라우저의 프로필과 로그인은 가져오지 않습니다.

## 동작과 제한

- CEF의 기본 Chrome UI를 사용합니다. `open`은 새 브라우저 창을 만들고 각 페이지를 CLI의 탭 ID로 구분합니다. 별도 디자인의 탭 UI는 아직 없습니다.
- 앱이 탭을 유지합니다. CLI는 요청 한 번을 보내고 종료합니다. 첫 버전은 여러 CLI 요청을 앱 전체에서 순서대로 처리합니다.
- 모든 `read`는 새로운 요소 참조를 발급합니다. 이전 `read`의 참조와 페이지 이동 전 참조는 만료됩니다. 실패한 클릭이나 입력은 자동 재시도하지 않습니다.
- 페이지별 격리된 JavaScript 실행 영역에서 DOM을 읽습니다. 비밀번호 입력값과 숨긴 입력값은 반환하지 않습니다. 본문은 12,000자, 조작 요소는 120개로 제한합니다.
- 첫 버전의 클릭과 입력은 DOM 이벤트입니다. 실제 사용자 입력과 같은 trusted 이벤트가 아닙니다. Enter는 입력 요소의 폼 제출을 요청하며, 결과는 다시 `read`해서 확인해야 합니다. 다른 키, iframe 내부, shadow DOM, 캔버스 조작은 미지원입니다.
- 로컬 소켓은 소유 사용자와 권한을 확인합니다. 소켓은 0600, 부모 폴더는 0700이어야 합니다. Chromium sandbox를 켜고 외부 디버깅 포트는 열지 않습니다.
- API는 로컬 앱 사용자를 위한 것입니다. 여러 AI 에이전트 사이의 별도 세션 권한, 사용자 조작 감지와 자동 중단은 아직 없습니다. 로그인한 페이지를 외부 모델에 전달하는 정책은 AI 연결 단계에서 추가해야 합니다.
- 웹페이지 본문은 비신뢰 데이터입니다. 페이지에 적힌 지시를 CLI 실행 권한으로 취급하면 안 됩니다.

## 검증

```bash
export PATH="$PWD/.cache/bin:$PATH"
export CEF_PATH="$PWD/.cache/cef"
cargo test --locked --offline --lib
node --test tests/dom_bridge.test.mjs
python3 tests/smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
```

Node는 DOM 단위 테스트에만 사용합니다. 앱과 CLI 실행에는 필요하지 않습니다.

통합 테스트는 실행 중인 앱에 접속합니다. 자체 로컬 HTTP 페이지를 띄워 페이지 읽기, 클릭, 입력, 실제 폼 탐색, 참조 만료, 비밀번호 제외, 격리 실행, 스크린샷을 확인합니다. 테스트가 만든 탭만 닫고 기존 탭은 남깁니다. 외부 사이트의 전체 호환성을 검증하는 테스트는 아닙니다.

2026-10-08 사용자가 앱 전용 키체인 접근을 승인한 뒤, 동일한 최종 릴리스 바이너리를 Documents 안의 전용 프로필로 두 번 실행해 로컬 통합 테스트 7개 항목을 모두 통과했습니다. 영속 쿠키, 세션 쿠키, localStorage가 재시작 후 유지되고 두 번 모두 정상 종료되는 것도 확인했습니다. 강제 종료는 없었습니다. Rust 단위 테스트 11개와 모의 DOM 테스트 9개도 통과했습니다.

최종 빌드에서 네이버 본문 읽기, `가벼운 브라우저` 한글 입력, Enter 검색, 검색 결과 URL과 본문 읽기, PNG 저장을 확인했습니다. 승인 후 실행 파일을 변경하지 않았습니다. 이어 별도 빈 프로필에서 네이버 접속을 포함해 앱을 두 번 재시작했고, 두 번 모두 페이지 읽기·PNG 캡처·정상 종료가 통과했습니다.

이 검증의 JSON 보고서와 스크린샷은 개발 환경의 `artifacts/`에 보관하며 Git에는 포함하지 않습니다. 위 결과는 개발 환경에서 실행한 기록이고 GitHub CI 결과는 아닙니다. 브라우저 프로필, 쿠키, 로그, 다운로드 캐시, 앱 번들도 업로드 대상에서 제외합니다.

기본 개발 프로필의 추가 확인에서는 한 차례 `open`이 45초 제한을 넘겼지만, 이후 NAVER 로딩 완료와 본문 읽기를 확인했습니다. 이 지연의 원인은 확정하지 않았으며, 후속 두 차례 재시작 검증에서는 재현되지 않았습니다.

실제 사이트 로그인 유지, 이전 `profile`의 데이터 이전, 결제, 다운로드, DRM 콘텐츠, 코드 변경 후 재서명에 대한 무승인 실행은 검증하지 않았습니다.

재시작 검증은 존재하지 않는 전용 데이터 경로를 사용합니다. 테스트가 만든 프로필과 결과 보고서를 남깁니다.

```bash
python3 tests/restart_smoke.py --app 'dist/Light Browser.app' \
  --cli target/release/lightbrowser --data-dir .local-data/restart-new
```

## 소스 구성

- `src/browser.rs`: CEF 수명주기, 명령 처리, 내부 DevTools API 연결
- `src/protocol.rs`, `src/cli.rs`: 로컬 소켓과 CLI
- `src/dom_bridge.js`: DOM 스냅샷과 요소 조작
- `src/mac.rs`, `src/bin/helper.rs`: macOS 앱 이벤트와 CEF sandbox helper
- `scripts/`: 의존성 준비, 앱 번들 생성과 실행

런타임은 CEF `156.0.1+g8db2f36+chromium-156.0.8078.17` beta, API는 `15600`으로 고정했습니다. 공식 cef-rs 생성기로 macOS 바인딩을 다시 만든 로컬 파생본을 `vendor/cef-rs`에 포함했습니다. 배포된 cef-rs 156 정식 패키지가 아니며 [생성 근거와 재현 방법](vendor/cef-rs/README.local.md)을 기록했습니다. 앱 시작 시 실제 API 해시를 확인하고, 번들 생성 전에 헤더·메타데이터·프레임워크 버전을 교차 확인합니다.
