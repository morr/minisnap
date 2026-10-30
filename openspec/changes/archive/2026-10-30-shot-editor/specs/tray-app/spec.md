# Spec Delta

## MODIFIED Requirements

### Requirement: Меню программы
Меню иконки SHALL содержать пункты «Capture Frozen Area», «Capture and Edit», «Open Latest Image», разделитель, «Preferences», разделитель и «Quit». Пункты захвата и открытия SHALL отображать справа серым цветом текущие комбинации клавиш этих действий; «Quit» SHALL отображать ⌘Q и завершать приложение.

#### Scenario: Состав меню
- **WHEN** пользователь открывает меню иконки
- **THEN** меню содержит пункты «Capture Frozen Area», «Capture and Edit» и «Open Latest Image» (каждый с текущим хоткеем справа серым), «Preferences», «Quit» (с ⌘Q)

#### Scenario: Хоткей в меню синхронен настройкам
- **WHEN** пользователь изменил хоткей действия в настройках
- **THEN** соответствующий пункт меню показывает новую комбинацию без перезапуска приложения
