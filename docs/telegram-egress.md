# Telegram egress / Исходящие подключения к Telegram

## Русский

Без поля `telegram_egress` daemon подключается к `api.telegram.org` напрямую. Чтобы настроить другие маршруты, добавьте в `config.json` от одного до четырёх элементов в порядке попыток. В следующем примере daemon сначала пробует локальный SOCKS-прокси, затем WireGuard.

```json
{
  "telegram_egress": {
    "routes": [
      {
        "type": "vless",
        "proxy_credentials_file": "/etc/want-garde/telegram-proxy"
      },
      {
        "type": "wireguard",
        "interface": "wg0",
        "dns_server": "10.42.0.53:53"
      }
    ]
  }
}
```

- `vless` использует SOCKS5, который уже запущен локальным VLESS-клиентом. В отдельном файле укажите одну строку вида `socks5h://127.0.0.1:1080`. Если прокси требует авторизацию, укажите в URL непустые имя и пароль с percent-encoding. Длина обоих полей после percent-декодирования ограничена 233 байтами суммарно. Допустим числовой loopback-адрес, включая `[::1]`, с явным портом. DNS-запрос для Telegram выполняет прокси.
- `wireguard` и `tailscale` используют готовый Linux-интерфейс (например, `wg0` или `tailscale0`). Укажите имя интерфейса и числовой адрес DNS-сервера с портом, доступный через этот интерфейс. Для IPv6 заключите адрес в квадратные скобки. DNS и соединение с Telegram привязаны к заданному интерфейсу. Host DNS для этих маршрутов не используется.
- `{"type":"direct"}` использует сетевую маршрутизацию и DNS хоста. В настроенной цепочке прямое подключение доступно только при наличии этого элемента. Daemon не использует прокси из переменных окружения.

Владелец файла SOCKS5 URL должен быть `root` или пользователь сервиса. Например, используйте `root:want-garde` с правами `0640` в каталоге `root:want-garde` с правами `0750`. Файл должен отличаться от конфигурации и файлов токенов. Укажите только путь к файлу в `config.json`. Команда `--check-config` от имени пользователя сервиса проверяет чтение и формат файла без подключения к прокси. Если любой из файлов не читается или содержит неверный URL, сервис отклоняет конфигурацию при запуске.

Интерфейс, DNS, маршруты и локальный VLESS-клиент настраивает администратор VPS. Daemon запускается от пользователя `want-garde` без root и дополнительных capabilities. Проверьте, что ядро разрешает непривилегированному процессу привязку сокета к интерфейсу. Для Tailscale нужен Linux-интерфейс и маршрут до адресов Telegram через подготовленный узел выхода.

Следующий маршрут используется при ошибке подключения до передачи HTTP-запроса. Если подтверждение отправки потерялось, следующую попытку назначает SQLite outbox. Она сохраняет ID доставки и исходный текст. Ответ Telegram с ограничением частоты или ошибкой авторизации действует на запрос целиком и не переключает маршрут. Все методы Telegram используют эту цепочку, включая транспорт для загрузки и скачивания файлов. Передача файла ограничена 8 МиБ. Файловые методы не добавляют команд владельца.

Если Telegram недоступен, проверьте журнал `want-garde.service` и локальную команду `--delivery-status`. `telegram.route_fallback` содержит номер и тип маршрута и код ошибки. `telegram.proxy_connect` означает ошибку подключения через прокси, `telegram.tunnel_interface` означает отказ привязки DNS-сокета, а `telegram.tunnel_connect` означает ошибку подключения через туннель. `telegram.dns` и `telegram.tls` отмечают выявленные ошибки DNS и TLS. `telegram.network` означает сетевую ошибку без точного диагноза, а `telegram.api` и другие коды Telegram описывают ответ сервера. URL, токены, адреса маршрутов и пароли в этих сообщениях отсутствуют.

Для отката восстановите предыдущую конфигурацию или удалите `telegram_egress`, если прямое подключение допустимо. Проверьте конфигурацию и перезапустите сервис. Очередь доставок хранится в прежней базе SQLite. См. [доставку и восстановление](delivery.md).

## English

If `telegram_egress` is absent, the daemon connects directly to `api.telegram.org`. Add one to four routes in order of preference under `telegram_egress.routes` in `config.json`. The example above tries a local SOCKS proxy first, then WireGuard.

- `vless` uses a SOCKS5 listener provided by a local VLESS client. Put one line such as `socks5h://127.0.0.1:1080` in a separate file. If the proxy requires authentication, put a nonempty username and password in the URL. The fields may be percent-encoded and total at most 233 bytes after decoding. Use a numeric loopback address, including `[::1]`, and an explicit port. The proxy resolves the Telegram hostname.
- `wireguard` and `tailscale` use a prepared Linux interface such as `wg0` or `tailscale0`. Specify the interface name and a numeric DNS server address and port reachable through it. Enclose IPv6 addresses in brackets. DNS and Telegram connections bind to that interface. These routes bypass the host resolver.
- `{"type":"direct"}` uses host routing and DNS. Include this entry in a configured chain only if direct access is intended. The daemon ignores proxy environment variables.

The SOCKS5 URL file must be owned by `root` or the service user. For example, use `root:want-garde` with mode `0640` in a `root:want-garde` directory with mode `0750`. Keep it separate from the configuration and token files. Put only its path in `config.json`. Run `--check-config` as the service user to verify access and format without connecting to the proxy. Startup rejects the configuration if any credential file is unreadable or contains an invalid URL.

The VPS administrator configures interfaces, DNS, routes, and the local VLESS client. The daemon runs as `want-garde` without root or extra capabilities. Verify that the Linux kernel permits unprivileged processes to bind sockets to the interface. Tailscale requires a Linux interface and a route to Telegram through the prepared exit node.

The client tries the next route after a connection failure before it transmits the HTTP request. If a send loses its acknowledgement, the SQLite outbox schedules another attempt with the same delivery ID and text. Telegram rate-limit and authorization responses apply to the request and stop route switching. All Telegram methods use the chain, including file upload and download. File transfers are limited to 8 MiB. The file methods add no owner commands.

If Telegram is unreachable, check the `want-garde.service` journal and local `--delivery-status`. `telegram.route_fallback` records the route position, route type, and error code. `telegram.proxy_connect` marks a connection failure through the proxy, `telegram.tunnel_interface` marks a DNS socket binding failure, and `telegram.tunnel_connect` marks a connection failure through the tunnel. `telegram.dns` and `telegram.tls` identify DNS and TLS failures when detected. `telegram.network` is a network failure without a narrower diagnosis. `telegram.api` and other Telegram codes describe server responses. These messages contain no URLs, tokens, route addresses, or passwords.

To roll back, restore the previous configuration or remove `telegram_egress` if direct access is acceptable. Verify the configuration and restart the service. Queued deliveries stay in the existing SQLite database. See [delivery and recovery](delivery.md).
