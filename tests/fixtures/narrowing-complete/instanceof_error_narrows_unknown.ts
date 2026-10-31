function messageOf(err: unknown): string {
    if (err instanceof Error) {
        return err.message;
    }
    return "unknown error";
}
