def create_style(string: str) -> str:
    return f":white_check_mark: [green]{string}[/green]"


def created() -> str:
    return create_style("Created")


def updated() -> str:
    return create_style("Updated")


def saved() -> str:
    return create_style("Saved")


def included() -> str:
    return create_style("Included")


def excluded() -> str:
    return create_style("Excluded")


def delete_style(string: str) -> str:
    return f":x: [red]{string}[/red]"


def deleted() -> str:
    return delete_style("Deleted")


def name_style(string: str) -> str:
    return f"[cyan]{string}[/cyan]"


def scope_style(string: str) -> str:
    return f"[bold]{string}[/bold]"


def global_() -> str:
    return scope_style("global")


def local() -> str:
    return scope_style("local")


def sync_style(string: str) -> str:
    return f":recycle: [blue]{string}[/blue]"


def syncing() -> str:
    return sync_style("Syncing")


def synced() -> str:
    return sync_style("Synced")


def set_() -> str:
    return sync_style("Set")
