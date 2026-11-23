# Telegram access / Доступ в Telegram

## Настройка доступа

Владелец управляет доступом командой `/access` в личном чате. На VPS те же настройки доступны root через `want-garde access`. Оба способа меняют одну политику в SQLite. Доступ сохраняется после перезапуска.

Выдавайте права по числовым ID пользователя и чата, полученным из доверенного источника. У группы отрицательный ID. В личном чате ID пользователя и чата совпадают. Роль администратора и участие в группе не дают прав. Для владельца в группе тоже требуется явное разрешение.

| Разрешение      | Команды                              |
| --------------- | ------------------------------------ |
| `status.read`   | `/status`, кнопка обновления статуса |
| `events.read`   | `/recent`, `/history`, `/stats`      |
| `geoip.read`    | `/geoip`                             |
| `bans.read`     | `/bans`                              |
| `services.read` | `/services`                          |
| `ports.read`    | `/ports`                             |
| `xui.read`      | `/xui`                               |

Команды `/access`, `/language`, `/notifications`, `/delivery`, `/ssh` и `/health` доступны только владельцу в личном чате. Разрешения действуют только на команды и их ответы. Получатели уведомлений по-прежнему задаются в конфигурации.

Пример для личного чата владельца. Замените ID на свои.

```text
/access list
/access grant 234567890 -1001234567890 status.read
/access grant 234567890 234567890 geoip.read
/access group -1001234567890 group
/access group -1001234567890 private
/access revoke 234567890 -1001234567890 status.read
```

Для делегированных команд используется поддерживаемый язык профиля Telegram или `default_locale`.

По умолчанию ответ на команду из группы приходит в личный чат автора команды. Пользователь должен открыть этот личный чат до запроса. Если Telegram не принимает личное сообщение, Want Garde не отправляет его в группу. Режим `group` открывает ответы всем, кто видит соответствующий чат или тему. В форуме ответ приходит в тему команды. Список показывает выданные разрешения и группы в режиме `group`. Поддерживается до 48 разрешений и 24 групп в режиме `group`.

Кнопка обновления действует пять минут, начиная с постановки ответа в очередь. Ею может воспользоваться только получатель в чате, где появилась кнопка. Если кнопка пришла в личный чат после команды из группы, при нажатии повторно проверяется исходное разрешение для группы. Каждая кнопка срабатывает один раз.

Отзыв права и смена режима доставки отменяют ответы, которые ещё не переданы Telegram. Повторная выдача права не восстанавливает старые ответы или кнопки. После начала HTTP-попытки доставка уже может состояться. Сообщения, которые Telegram уже принял, остаются доступными получателям.

### Управление с VPS

```sh
sudo want-garde access list
sudo want-garde access grant 234567890 -1001234567890 status.read
sudo want-garde access group -1001234567890 private
sudo want-garde access revoke 234567890 -1001234567890 status.read
```

Для другой конфигурации добавьте `--config /absolute/path/config.json` после `access`. По умолчанию CLI использует `/etc/want-garde/config.json`. Демон проверяет UID клиента через Unix socket `<state_dir>/access.sock`. Доступ разрешён только UID 0 при запущенном демоне, в том числе при недоступном Telegram. Если очередь доставки заполнена и команда владельца в Telegram ждёт обработки, используйте CLI. На Linux полный путь к файлу сокета должен занимать не более 107 байт. Стандартный путь `/var/lib/want-garde/access.sock` помещается в этот предел. Если сокет не открывается, в журнал попадает код `access.socket`, а мониторинг и Telegram продолжают работать.

Аудит хранит время, источник, числовые ID, функцию и код решения. Текст команды и данные VPS в эти записи не попадают. Сохраняется до 4096 записей в пределах `retention.action_audit_days`. Если аудит нельзя записать, изменение политики или защищённый ответ блокируется.

## Set up access

The owner manages access with `/access` in a private chat. On the VPS, root can update the same SQLite policy with `want-garde access`. Grants survive a restart.

Use numeric user and chat IDs from a trusted source. Group IDs are negative. In a private chat, the user and chat IDs are identical. Group membership and administrator roles confer no rights. The owner also needs an explicit grant in a group.

The table above lists each capability and its commands. Only the owner can use `/access`, `/language`, `/notifications`, `/delivery`, `/ssh`, and `/health` in private. Grants apply to commands and their replies. Notification recipients still come from configuration.

Start with `/access list` to see grants and groups configured for `group` delivery. The Telegram examples above show how to grant and revoke access in both chat types. Use your own IDs. The policy allows up to 48 grants and 24 groups in `group` mode.

Delegated commands use the Telegram profile language when supported and `default_locale` otherwise.

By default, the requester gets a private reply to a group command. The requester must open a private chat with Want Garde first. If Telegram refuses the private reply, Want Garde never sends it to the group. In `group` mode, everyone who can read the chat or topic can see the reply. In forums, the reply goes to the topic that received the command.

A status refresh button is valid for five minutes from the time the reply enters the queue. It works once, for the original requester in the chat where the button was sent. A button sent privately after a group command still requires the original group grant.

Revoking a grant or changing delivery mode cancels replies before Telegram dispatch. Granting access again leaves earlier replies and buttons invalid. An HTTP attempt that has already started may still deliver. Changes cannot recall messages Telegram has accepted.

### Manage access on the VPS

The shell examples above run the same operations with `sudo`. Add `--config /absolute/path/config.json` after `access` for a custom configuration. The default is `/etc/want-garde/config.json`. The daemon checks the connecting UID over `<state_dir>/access.sock` and accepts only UID 0. The daemon must be running. Telegram can be offline. If the delivery queue is full and the owner's Telegram command is waiting, use the CLI. On Linux, the socket pathname must fit in 107 bytes. The default `/var/lib/want-garde/access.sock` fits. If the socket cannot open, the daemon logs `access.socket` and continues monitoring and serving Telegram.

Audit entries record time, source, numeric IDs, capability, and a decision code. They omit command text and VPS data. The policy keeps up to 4096 entries within `retention.action_audit_days`. If the audit write fails, it blocks the policy change or protected reply.
