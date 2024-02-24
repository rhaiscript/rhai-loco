fn login() {
    import "hello/world" as world;

    debug(`Trying to login with user = ${this.user} and password = ${this.password}`);

    throw `${world::say_hi(this.user)}`;
}