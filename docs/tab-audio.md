# 탭 음소거

현재 탭의 메뉴와 현재 공간의 펼친 사이드바 행에서 음소거 설정을 바꾼다. 구현과 실제 실행 결과는 아래 및 [작업 기록](work-log.md)에서 구분한다. 음소거 설정 확인을 실제 스피커 출력 측정으로 설명하지 않는다.

## 사용자 동작

보기 메뉴의 `현재 탭 음소거`는 현재 표시한 탭의 출력 설정을 토글한다. 체크 표시는 CEF에서 확인한 음소거 설정이며 현재 소리가 재생 중이라는 뜻이 아니다. 음소거가 확인된 탭은 기존 아이콘을 `speaker.slash`로 바꾸고 접근성 안내에 음소거 설정을 덧붙인다. 로딩과 AI 상태는 안내 문구에 유지한다. 사용자 공간과 AI 공간 모두 사용할 수 있지만 공간 목록과 검색 등 전체 오버레이, 닫힌 탭, renderer 오류, 비활성 창과 입력기 조합 중에는 실행하지 않는다.

AI 작업을 인계하거나 중지하지 않고 페이지를 탐색하지 않는다. 선택, 주소 초안과 입력 포커스를 직접 변경하지 않는다. 새 탭과 다시 연 탭에 이전 설정을 복원하는 기능은 없다. 모든 공간은 기존 프로필을 공유하며 이 기능을 공간별 보안 격리로 설명하지 않는다.

2026-10-20 후속 구현은 펼친 사이드바의 각 탭에 음소거 버튼을 둔다. 현재 공간의 배경 탭을 선택하지 않고 토글하며 정확한 행의 버튼을 확인한다. 아직 값을 읽지 않은 탭은 클릭 시 실제 CEF 설정을 조회한다. 음소거 아이콘은 재생 여부를 나타내지 않는다. 접힌 사이드바는 기존 선택과 닫기 영역을 유지하기 위해 행 음소거 버튼을 숨긴다. 다른 공간에 있는 탭의 원격 음소거는 이 버튼의 범위가 아니다.

## 호출 흐름과 수명

`control_panel::ShellTarget.menuWillOpen`이 현재 탭, 공간과 탐색 세대, 작업 ID와 revision, 상태를 캡처한다. `browser::read_tab_audio`는 살아 있는 BrowserHost의 `is_audio_muted`를 읽고 탭 메타데이터에 마지막 확인값을 기록한다. 메뉴가 열린 뒤 validation이 반복돼도 다른 탭으로 대상을 교체하지 않는다. 실행할 때 저장한 대상과 예상 설정을 한 번 꺼낸다.

`browser::set_tab_audio_muted`는 UI 현재성, 선택, 소속, 탐색 세대와 제어권을 검사하고 native getter를 다시 읽는다. 예상값이 달라졌다면 쓰지 않는다. UI와 browser 상태를 다시 확인한 뒤 `set_audio_muted`를 호출한다. 호출 뒤 탭이 여전히 유효한 경우만 getter로 설정 반영을 확인한다. setter 중 닫힌 탭의 보관된 host를 재조회하지 않는다. 재진입으로 대상이 바뀌거나 설정이 반영되지 않은 경우에는 변경 시도와 결과 확인 실패를 구분한다. STATE와 SHELL borrow를 CEF 호출에 걸쳐 유지하지 않는다.

행 경로는 `capture_tab_audio_row_target`으로 대상을 캡처하고 같은 `read_tab_audio`, `set_tab_audio_muted`를 재사용한다. 기존 메뉴는 선택된 탭만 허용한다. 행은 현재 공간의 배경 탭도 허용하되 캡처 당시의 선택, 공간, 소속, 탐색 세대와 작업 ID, revision, 상태, 저장된 Browser 동일성을 재검증한다. 행 버튼을 누르는 과정에서 select, handoff와 페이지 focus를 호출하지 않는다.

탭에는 `Option<bool>` 하나, 메뉴에는 마지막 대상과 예상값 한 개, 재진입 guard 한 개를 둔다. 새 탭은 확인 전 `None`이다. 기존 탭 행과 아이콘을 재사용하고 행마다 네이티브 버튼 하나를 추가한다. PCM 수집, 소리 분석, 타이머와 worker, 웹 UI, 저장 파일은 추가하지 않는다. 이 구조는 실제 RAM 절감 측정값이 아니다.

## 고정된 CEF와 Chromium 계약

설치된 CEF 156.0.1의 커밋은 `8db2f36735f06aacaf1bab832cb08da9d9445c55`, Chromium은 `156.0.8078.17`이다.

- CEF getter는 UI 스레드 전용이다. setter도 UI 스레드에서 같은 WebContents에 위임하도록 사용한다. [CEF browser host](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/browser_host_base.cc#L876)
- 설정은 해당 WebContents와 그 guest의 출력 그룹에 적용하며 같은 프로필이나 origin의 다른 독립 탭을 수정하지 않는다. 오디오 capture에는 영향을 주지 않는다. [Chromium 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/web_contents/web_contents_impl.cc#L2904), [공개 계약](https://github.com/chromium/chromium/blob/156.0.8078.17/content/public/browser/web_contents.h#L955)
- 같은 WebContents의 탐색에서 설정이 유지되고 새 factory는 unmuted로 시작한다. 프로필에 영속화하는 경로는 아니다. setter 직후 getter는 UI 설정 반영을 확인하며 실제 출력 장치 처리는 IO 작업으로 전달된다. [기본값](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/media/forwarding_audio_stream_factory.h#L219), [IO 전달](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/media/forwarding_audio_stream_factory.cc#L265)
- 설치된 공개 헤더와 Rust 바인딩에는 현재 audible 상태 getter나 변경 콜백이 없다. AudioHandler의 loopback은 `mute_source=true`를 사용하므로 표시만을 위해 붙이면 기본 출력에 영향을 줄 수 있다. 이 구현에서는 연결하지 않는다. [CEF audio loopback](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/audio_loopback_stream_creator.cc#L70)

## 남은 검증

엔진을 초기화하지 않은 fake CEF host 단위 검사는 getter와 setter, 대상 만료, 재진입과 실패 처리를 확인한다. 실제 AppKit 메뉴 콜백 순서, 체크와 아이콘 표시, 주소 초안과 IME, 숨긴 AI 탭과 같은 origin의 다른 탭 출력, 문서 이동과 탭 닫기, 실제 스피커 출력과 장시간 RAM은 별도 실행 검증이 필요하다. 최신 검사 결과와 생성물 정리는 [작업 기록](work-log.md)에 남긴다.

후속 행 버튼은 통합 빌드 후 다음 흐름으로 확인한다. 이 목록은 실행 결과가 아니다.

1. 같은 공간의 두 사용자 탭에서 배경 행만 음소거하고 선택 탭과 주소 초안을 확인한다.
2. 같은 origin의 다른 탭 설정이 바뀌지 않는지, 다시 누르면 해당 행만 해제되는지 확인한다.
3. AI 공간의 배경 행에서 음소거하고 작업 active 상태와 revision, 선택을 확인한다.
4. 접힌 사이드바의 기존 선택과 닫기 영역, 펼친 뒤 버튼과 상태를 확인한다.
5. 기존 보기 메뉴에서 현재 탭 음소거가 계속 동작하는지 확인한다.
6. 무음 fixture의 수동 시계 기록으로 페이지 실행이 이어지는지 확인한다. 네이티브 설정과 실제 출력 측정은 구분한다.

## 검증 상태

2026-10-20 기준 위 목록의 확인 1은 v25 빌드에서 실제 AppKit으로 확인했다(`artifacts/e2e-v25-tab-audio-20261020/`). 확인 2부터 5는 행 버튼 클릭 등 네이티브 조작이 필요해 아직 실행하지 않았다. 확인 6은 음소거 없이 숨긴 탭의 페이지 시계가 진행됨만 CLI로 관찰했고 음소거 상태의 비교는 미실행이다(`artifacts/m0-tab-audio-20261020/`). 상세는 [작업 기록](work-log.md)에 있다.
