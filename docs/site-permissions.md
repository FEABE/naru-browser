# 사이트 권한

다운로드 권한은 기존 파일 패널에서 관리한다. 위치, 알림, 카메라와 마이크 요청은 같은 브라우저 창의 네이티브 사이트 권한 패널에서 확인한다. 별도 작업창, 웹 UI renderer, DB나 상주 프로세스를 추가하지 않는다.

## 요청과 결정

배경 AI 요청은 사용자 공간, 선택 탭과 입력 포커스를 자동으로 바꾸지 않는다. 사이드바에 대기 상태를 표시하고 사용자가 패널을 열어 요청 사이트, 작업과 탭, 필요한 권한을 확인한다. 허용과 취소는 네이티브 버튼에서만 수행한다. CLI는 대기 상태만 읽고 권한을 승인할 수 없다.

| 요청 | CEF 경로 | 허용의 범위 |
| --- | --- | --- |
| 위치, 알림 | `OnShowPermissionPrompt` | **이 사이트 허용**. Chromium 프로필에 저장되는 사이트 설정이다. 모든 공간이 같은 프로필을 사용한다. |
| 카메라, 마이크 | `OnRequestMediaAccessPermission` | **이번 요청 허용**. 해당 미디어 요청의 정확한 비트 조합을 이어 실행한다. macOS의 별도 기기 권한은 이 승인으로 대체하지 않는다. |
| 화면 공유, 그 밖의 미지원 권한 | 각각의 콜백 | 요청을 취소하고 미지원 이유를 표시한다. 화면이나 기기를 임의로 선택하지 않는다. |

취소는 현재 요청을 끝낸다. 사이트를 영구 차단하는 버튼과 같은 의미가 아니다. 위치와 알림의 현재 저장 상태는 열린 페이지의 정확한 origin으로 조회한다. 초기화는 그 origin의 두 설정만 기본값으로 되돌리고, 이미 진행 중인 위치 조회나 미디어 스트림의 종료를 보장하지 않는다.

macOS 번들에는 위치 접근 목적을 설명하는 `NSLocationUsageDescription`을 포함한다. 이 문자열과 사이트 승인은 OS 권한을 자동으로 부여하지 않는다. [Apple의 macOS 위치 사용 설명 계약](https://developer.apple.com/documentation/bundleresources/information-property-list/nslocationusagedescription)을 따른다.

카메라와 마이크를 허용한 뒤 사이트가 받은 스트림의 수명은 사이트와 Chromium이 관리한다. AI 중지와 작업 인계에서 대기 요청을 취소하는 동작을 이미 시작된 캡처의 중지로 설명하지 않는다.

`OnMediaAccessChange`에서 받은 카메라와 마이크 스트림 활성 상태는 탭별로 표시한다. 이 콜백은 시작된 스트림의 등록 수명을 반영하며 개별 트랙의 음소거, 실제 녹음이나 프레임 수신을 측정하지 않는다. 작업 인계나 권한 초기화만으로 표시를 지우지 않는다. 엔진의 상태 변경이나 탭과 렌더러 종료에서 갱신한다.

## 출처와 수명

요청은 탭, 원래 문서, 작업 소유자와 revision, 요청 세대에 묶는다. 승인 직전에 같은 대상인지 다시 확인한다. 문서 이동, 하위 프레임 이동과 제거, 탭 종료, 렌더러 종료와 제어권 변경은 대기 요청을 취소한다. CEF 콜백의 보관, 실행과 해제는 공유 상태의 borrow 밖에서 수행한다.

일반 권한 콜백에는 요청 프레임이 없다. CEF가 전달하는 requesting origin을 표시하되 메인 페이지가 요청자라고 추정하지 않는다. 미디어 콜백에는 frame이 있지만 고정 CEF 구현은 원 프레임을 찾지 못하면 main frame으로 대체한다. 따라서 이 값만으로 요청 프레임을 정확히 증명하지 않는다. 하위 프레임이 바뀌면 탭 전체의 대기 요청을 보수적으로 취소한다.

Chromium은 숨겨진 탭의 일반 권한 프롬프트를 보류할 수 있다. 미디어 요청도 스레드 사이의 비동기 전달을 거친다. 두 경우 모두 Naru 콜백이 도착하기 전에 제어권이 바뀌었을 수 있다. 제어권이 바뀐 같은 문서의 사이트 권한 요청은 새 문서 탐색 전까지 거절한다. 그 뒤에 새로 발생한 정상 요청도 다시 페이지를 열어야 할 수 있다. 요청자가 바뀐 것처럼 재분류해 허용하지 않는다.

## 자원과 검증

대기 요청은 전체 최대 4개, 탭마다 최대 1개다. 요청 식별자는 다시 사용하지 않는다. origin과 표시 문자열을 제한하고 필요한 콜백과 메타데이터만 메모리에 보관한다. 대기 수와 문자열 상한은 Chromium 전체 메모리 상한을 뜻하지 않는다.

[`tests/fixtures/site-permissions.html`](../tests/fixtures/site-permissions.html)은 버튼으로만 권한을 요청한다. 알림을 발송하지 않으며 위치 좌표와 기기 이름을 기록하지 않는다. 미디어 승인이 성공하면 받은 트랙을 즉시 종료한다. 실제 기기 허용은 별도 확인 없이는 검증 범위에 넣지 않는다.

v22는 좁은 단위 검사 4개와 release 빌드를 통과했다. 실제 로컬 CEF에서 알림 요청의 접수와 네이티브 패널의 출처, 저장 범위, 버튼 표시까지 확인했다. 커밋을 우선하라는 사용자 요청에 따라 허용, 취소와 초기화 결과는 아직 확인하지 않았다. 아래 1번도 전체 통과가 아니며 상세 근거는 [v22 E2E 기록](ego-parity-e2e.md#2026-10-20-v22-사이트-권한-구현과-부분-확인)을 따른다.

같은 통합 빌드에서 이어 확인할 목록은 다음과 같다.

1. 사용자 탭에서 알림 요청, 패널과 출처 확인, 취소 후 페이지 결과를 확인한다.
2. 자체 프로필의 알림 허용과 페이지의 `Notification.permission`, 새 페이지에서 저장 상태와 초기화 결과를 확인한다. 실제 알림은 발송하지 않는다.
3. 배경 AI 요청에서 사용자 공간과 선택을 확인한다. 엔진이 숨긴 요청을 Naru가 이미 접수했다고 기록하지 않는다.
4. 대기 요청 중 인계, 재개, 문서 탐색과 탭 종료 후 오래된 승인 버튼으로 결과가 전달되지 않는지 확인한다.
5. 위치와 카메라, 마이크 요청은 취소 결과를 확인한다. 실제 좌표 조회나 기기 녹화 성공과 구분한다.
6. 검색, 북마크와 다운로드 패널 전환, 주소 초안, Escape, 키보드 포커스, 좁은 창에서 요청 출처와 버튼을 확인한다.
7. 같은 출처 프레임의 이동과 제거, 요청 한도, 작업 종료와 앱 종료에서 대기 정리를 확인한다.

소스 구현, 단위 검사, 로컬 CEF와 네이티브 UI, 외부 사이트와 실제 기기 검증은 [작업 기록](work-log.md)에 따로 남긴다. 계획 목록을 성공 결과로 사용하지 않는다.

## 고정 엔진 근거

현재 기준은 CEF `8db2f36735f06aacaf1bab832cb08da9d9445c55`, Chromium `156.0.8078.17`이다. 로컬 헤더와 공식 소스의 URL, 해시와 확인한 행은 [조사 기록](../artifacts/site-permissions-cef-sources.json)에 남긴다.

- [CEF 권한 콜백 계약](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/include/cef_permission_handler.h)은 일반 프롬프트와 미디어 요청, 기본 거절과 dismiss 콜백을 구분한다.
- [CEF 일반 프롬프트](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/permission_prompt.cc)는 일회성 허용을 노출하지 않는다. [Chromium 권한 저장](https://github.com/chromium/chromium/blob/156.0.8078.17/components/permissions/permission_context_base.cc)의 일반 허용은 저장 설정으로 이어진다.
- [Chromium 프롬프트 관리자](https://github.com/chromium/chromium/blob/156.0.8078.17/components/permissions/permission_request_manager.cc)는 비활성 탭의 프롬프트를 뒤로 미룬다. [미디어 요청 전달](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/renderer_host/media/media_stream_ui_proxy.cc)에는 IO에서 UI로 보내는 비동기 구간이 있다.
- [CEF 미디어 요청](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/media_access_query.cc)은 요청 프레임 대체와 콜백 일회 소비를 확인한 근거다. 이 경로의 성공을 OS 권한이나 실제 캡처 성공으로 대신하지 않는다.
